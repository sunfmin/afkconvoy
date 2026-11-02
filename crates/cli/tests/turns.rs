//! A turn's observable contract, through the real CLI and git remote.
use crate::common::{World, git};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

const LINE: &str = "refs/convoy/train/line/main";

fn quote(path: &Path) -> String {
    afkconvoy_core::line::Line::new().word(&path.to_string_lossy()).own()
}

fn until(done: impl Fn() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(
            began.elapsed() < Duration::from_secs(15),
            "the expected transition did not happen"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn start(world: &World, dir: &Path, cache: &Path, issue: u64) -> Child {
    world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), dir)
        .env("XDG_CACHE_HOME", cache)
        .env("AFKCONVOY_ISSUE", issue.to_string())
        .args(["gate", "--land"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(child: Child) -> Value {
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn seed(world: &World) {
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["recorded"], true, "{answer}");
}

fn workers_waiting(world: &World) -> usize {
    world.afk(&["doctor"]).json["host"]["exclusive"]["waiting"]
        .as_array()
        .map_or(0, |waiting| waiting.iter().filter(|request| request["lane"] == "worker").count())
}

#[test]
fn two_turns_land_in_order_with_a_record_of_each_landed_tree_and_no_line() {
    let world = World::new();
    let started = world.root.join("first-started");
    let release = world.root.join("release");
    let second_started = world.root.join("second-started");
    world.set_gate(&format!(
        "if test -f one.txt && test ! -f two.txt; then touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi; if test -f two.txt; then test -f one.txt || exit 9; touch {}; fi",
        quote(&started), quote(&world.root), quote(&release), quote(&second_started)));
    seed(&world);
    let one = world.worker(1, "one.txt", "one");
    let two = world.worker(2, "two.txt", "two");
    let _runner = world.host_held();
    let a = start(&world, &one, &world.cache(), 1);
    until(|| started.exists());
    let b = start(&world, &two, &world.cache(), 2);
    until(|| {
        world.afk(&["doctor"]).json["host"]["exclusive"]["waiting"]
            .as_array()
            .is_some_and(|w| w.iter().any(|request| !request["repo_lane"].is_null()))
    });
    assert!(!second_started.exists(), "a worker without its turn runs no gate");
    std::fs::write(&release, "").unwrap();
    let (a, b) = (finish(a), finish(b));
    assert_eq!(a["outcome"], "merged", "{a}");
    assert_eq!(b["outcome"], "merged", "{b}");
    assert_eq!(b["merged"], a["commit"], "the tip is read when its turn starts");
    assert!(!two.join("one.txt").exists(), "candidate merges preserve the source worktree");
    assert_eq!(world.remote(&["show", "refs/heads/main:one.txt"]), "one");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), b["commit"].as_str().unwrap());
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
    for answer in [&a, &b] {
        let commit = answer["commit"].as_str().unwrap();
        let tree = git(&world.clone, &["rev-parse", &format!("{commit}^{{tree}}")]);
        assert!(
            !world
                .remote(&[
                    "for-each-ref",
                    "--format=%(refname)",
                    &format!("refs/convoy/gate/{tree}-*")
                ])
                .is_empty()
        );
    }
}

#[test]
fn red_and_conflict_release_the_turn_without_joining() {
    let world = World::new();
    world.set_gate("test ! -f bad.txt");
    seed(&world);
    let red = world.worker(1, "bad.txt", "bad");
    let answer = world.afk_in(&red, &["gate", "--land"]).ok();
    assert_eq!(answer["status"], "red");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
    let one = world.worker(2, "README.md", "two");
    let two = world.worker(3, "README.md", "three");
    assert_eq!(world.afk_in(&one, &["gate", "--land"]).ok()["outcome"], "merged");
    let before = world.remote(&["rev-parse", "refs/heads/main"]);
    let answer = world.afk_in(&two, &["gate", "--land"]).ok();
    assert_eq!(answer["outcome"], "conflict", "{answer}");
    assert_eq!(git(&two, &["diff", "--name-only", "--diff-filter=U"]), "README.md");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), before);
}

#[test]
fn a_missing_base_record_is_gated_alone_before_any_worker_turn() {
    let world = World::new();
    let certifying = world.root.join("certifying");
    let release = world.root.join("release");
    let worker_gate = world.root.join("worker-gate");
    world.set_gate(&format!(
        "if test -f worker.txt; then touch {}; else touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi",
        quote(&worker_gate), quote(&certifying), quote(&world.root), quote(&release)));
    let worker = world.worker(1, "worker.txt", "one");
    let _runner = world.host_held();
    let child = start(&world, &worker, &world.cache(), 1);
    until(|| certifying.exists());
    let doctor = world.afk(&["doctor"]).json;
    let queue = &doctor["host"]["exclusive"];
    assert!(queue.get("train").is_none(), "{queue}");
    assert!(queue["repos"].as_array().is_some_and(|lanes| !lanes.is_empty()), "{queue}");
    assert!(!worker_gate.exists());
    std::fs::write(release, "").unwrap();
    assert_eq!(finish(child)["outcome"], "merged");
    assert!(worker_gate.exists());
}

#[test]
fn a_lock_issue_from_an_older_fleet_lands_its_fix_on_a_red_tip() {
    let world = World::new();
    // The base branch is red: its gate needs the file only the fix adds.
    world.set_gate("test -f fixed.txt");
    let fix = world.worker(7, "fixed.txt", "fixed");
    world.issue(7, &["fleet-lock"]);
    let mut child = start(&world, &fix, &world.cache(), 7);
    let began = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if began.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            panic!("the lock issue's turn waited for a green tip its own fix makes green");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let answer = finish(child);
    assert_eq!(answer["outcome"], "merged", "{answer}");
    assert_eq!(answer["recorded"], true);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), answer["commit"].as_str().unwrap());
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
}

fn heavy(world: &World) {
    let gate = afkconvoy_core::line::Line::new()
        .program(env!("CARGO_BIN_EXE_afkconvoy"))
        .fixed(" exclusive -- sh ./gate.sh")
        .own();
    world.set_gate(&gate);
    let script = format!(
        "#!/bin/sh\nset -e\nif test -f held.txt; then touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi\n",
        quote(&world.root.join("started")),
        quote(&world.root),
        quote(&world.root.join("release"))
    );
    crate::common::commit(&world.clone, "gate.sh", &script);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    seed(world);
}

#[test]
fn two_worker_lanes_bound_candidates_and_leave_capacity_available_across_repos() {
    let (a, b) = (World::new(), World::new());
    heavy(&a);
    heavy(&b);
    a.say_host_settings(json!({"gate_lanes": 2}));
    let one = a.worker(1, "held.txt", "one");
    let another = a.worker(2, "second.txt", "second");
    let other_repo = b.worker(3, "held.txt", "other repo");
    let _runner = a.host_held();
    let first = start(&a, &one, &a.cache(), 1);
    until(|| a.root.join("started").exists());
    let third = start(&b, &other_repo, &a.cache(), 3);
    until(|| b.root.join("started").exists());
    let second = start(&a, &another, &a.cache(), 2);
    until(|| workers_waiting(&a) == 1);
    assert!(!another.join("held.txt").exists(), "the second turn has not merged yet");
    let doctor = a.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["settings"]["in_force"]["gate_lanes"], 2);
    assert!(doctor["host"]["settings"]["in_force"].get("ignored").is_none());
    std::fs::write(a.root.join("release"), "").unwrap();
    std::fs::write(b.root.join("release"), "").unwrap();
    assert_eq!(finish(first)["outcome"], "merged");
    assert_eq!(finish(third)["outcome"], "merged");
    assert_eq!(finish(second)["outcome"], "merged");
    assert!(!another.join("held.txt").exists(), "source heads stay intact");
    assert_eq!(a.remote(&["show", "refs/heads/main:held.txt"]), "one");
}

#[test]
fn a_light_gate_takes_its_repo_turn_while_the_worker_lane_is_occupied() {
    let world = World::new();
    let other = World::new();
    world.set_gate("true");
    seed(&world);
    let worker = world.worker(1, "one.txt", "one");
    let _runner = world.host_held();
    let held = world.root.join("held");
    let release = world.root.join("release");
    let script = format!(
        "touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done",
        quote(&held),
        quote(&world.root),
        quote(&release)
    );
    let mut command = other.command(env!("CARGO_BIN_EXE_afkconvoy"), &other.clone);
    command.env("XDG_CACHE_HOME", world.cache());
    let heavy = command
        .args(["exclusive", "--", "sh", "-c", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    until(|| held.exists());
    assert_eq!(finish(start(&world, &worker, &world.cache(), 1))["outcome"], "merged");
    std::fs::write(release, "").unwrap();
    assert!(heavy.wait_with_output().unwrap().status.success());
}

#[test]
fn the_exclusive_bound_ends_the_whole_turn_and_releases_the_next_one() {
    let world = World::new();
    heavy(&world);
    world.say_host_settings(json!({"exclusive_seconds": 1}));
    let one = world.worker(1, "held.txt", "one");
    let two = world.worker(2, "second.txt", "two");
    let _runner = world.host_held();
    let first = start(&world, &one, &world.cache(), 1);
    until(|| world.root.join("started").exists());
    let next = start(&world, &two, &world.cache(), 2);
    let ended = first.wait_with_output().unwrap();
    assert_eq!(ended.status.code(), Some(124), "{}", String::from_utf8_lossy(&ended.stderr));
    let answer: Value = serde_json::from_slice(&ended.stdout).unwrap();
    assert!(answer["error"].as_str().unwrap().contains("exclusive bound of 1s"), "{answer}");
    assert_eq!(finish(next)["outcome"], "merged");
    assert!(!two.join("held.txt").exists(), "the red turn joined nothing");
}

#[test]
fn the_tip_uses_its_own_gate_command_and_each_turn_uses_the_merged_config() {
    let world = World::new();
    world.set_gate("true");
    let first = world.worker(1, "required.txt", "one");
    let second = world.worker(2, "second.txt", "two");
    crate::common::commit(
        &first,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: test -f required.txt\ngate_seconds: 600\n```\n",
    );
    let changed = world.afk_in(&first, &["gate", "--land"]).ok();
    assert_eq!(changed["outcome"], "merged", "{changed}");
    assert_eq!(changed["command"], "test -f required.txt");
    let merged = world.afk_in(&second, &["gate", "--land"]).ok();
    assert_eq!(merged["outcome"], "merged", "{merged}");
    assert_eq!(merged["command"], "test -f required.txt", "the merged config names the gate");
    assert_eq!(merged["recorded"], true);
}

#[test]
fn a_green_run_without_a_written_record_joins_nothing_and_keeps_the_pr_in_its_answer() {
    let world = World::new();
    world.set_gate("true");
    seed(&world);
    let worker = world.worker(1, "one.txt", "one");
    world.watch_pushes(Some("convoy/gate"));
    let answer = world.afk_in(&worker, &["gate", "--land"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], false);
    assert_eq!(answer["outcome"], "gate_red");
    assert_eq!(answer["issue"], 1);
    assert_eq!(answer["pr"], 10);
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
    world.watch_pushes(None);
    assert_eq!(world.afk_in(&worker, &["gate", "--land"]).ok()["outcome"], "merged");
}

#[test]
fn a_green_tip_whose_record_cannot_be_written_returns_a_retryable_error() {
    let world = World::new();
    world.set_gate("true");
    let worker = world.worker(1, "one.txt", "one");
    world.watch_pushes(Some("convoy/gate"));
    let error = world.afk_in(&worker, &["gate", "--land"]).error();
    assert!(error.contains("gated green without a record"), "{error}");
    world.watch_pushes(None);
    assert_eq!(world.afk_in(&worker, &["gate", "--land"]).ok()["outcome"], "merged");
}

fn red_base_repair(bound: bool, legacy_lock: Option<&str>) {
    let world = World::new();
    let certified = world.root.join("certified-red");
    let worker_gated = world.root.join("worker-gated");
    world.set_gate(&format!(
        "if test -f broken.txt && test ! -f repaired.txt; then pwd > {}; exit 1; fi; if test -f worker.txt; then touch {}; fi",
        quote(&certified), quote(&worker_gated)));
    if bound {
        world.repo(
            "acme/dev",
            &[(".claude/skills/fix/SKILL.md", "---\nname: fix\ndescription: fix\n---\n")],
        );
        let workflow = if legacy_lock == Some("worker") {
            // The product role can only use Codex; the operator has Claude.
            crate::common::commit(
                &world.root.join("github/acme/dev"),
                ".codex/hooks.json",
                r#"{"hooks":{"PreToolUse":[{"hooks":[]}]}}"#,
            );
            "```yaml\nstages:\n  - name: decision\n    by: dev\n    do: fix\n    on: {pass: done}\n```\n"
        } else {
            "```yaml\nstages:\n  - name: decision\n    by: human\n    on: {pass: done}\n```\n"
        };
        world.repo(
            "acme/team",
            &[
                ("team.md", "```yaml\nroles: {dev: acme/dev}\n```\n"),
                ("workflows/decision.md", workflow),
            ],
        );
        let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md"))
            .unwrap()
            .replace("base_branch: main\n", "base_branch: main\nteam: acme/team\n");
        crate::common::commit(&world.clone, "docs/agents/afkconvoy.md", &config);
        git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    }
    seed(&world);
    let worker = world.worker(1, "worker.txt", "normal work");
    crate::common::commit(&world.clone, "broken.txt", "outside change");
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    let before = world.remote(&["rev-parse", "refs/heads/main"]);
    let child = start(&world, &worker, &world.cache(), 1);
    until(|| {
        !world
            .remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/base/red/main"])
            .is_empty()
    });
    assert!(!worker_gated.exists(), "no ordinary turn starts on red");
    assert_ne!(std::fs::read_to_string(&certified).unwrap().trim(), worker.to_str().unwrap());
    if legacy_lock.is_some() {
        let mut labels = vec!["fleet-lock", "flow:decision", "stage:decision"];
        if legacy_lock == Some("human") {
            labels.push("ready-for-human");
        }
        world.issue(7, &labels);
        let before = world.afk(&["plan"]).ok();
        assert!(
            before["frontier"].as_array().unwrap().is_empty(),
            "old workflow blocks it: {before}"
        );
    }
    let tick = world.afk(&["tick", "--instance", "repair-host"]).ok();
    let issues = world.gh();
    let repair = issues["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| {
            if legacy_lock.is_some() {
                issue["number"] == 7
            } else {
                issue["title"].as_str().is_some_and(|s| s.starts_with("Repair the red"))
            }
        })
        .expect("the fleet opened a repair")["number"]
        .as_u64()
        .unwrap();
    if legacy_lock.is_some() {
        assert!(world.labels(7).contains(&"convoy-repair".into()));
        assert!(world.labels(7).contains(&"ready-for-agent".into()));
        assert!(!world.labels(7).contains(&"ready-for-human".into()));
        assert_eq!(issues["issues"].as_array().unwrap().len(), 2, "reuse the existing lock");
    }
    assert!(
        tick["did"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["do"] == "dispatch" && step["issue"] == repair),
        "{tick}"
    );
    let ordinary = child.wait_with_output().unwrap();
    assert!(
        !ordinary.status.success()
            || serde_json::from_slice::<Value>(&ordinary.stdout).unwrap()["outcome"] == "locked"
    );
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), before);
    let orca = world.orca();
    let checkout =
        orca["worktrees"].as_array().unwrap().iter().find(|w| w["linkedIssue"] == repair).unwrap()
            ["path"]
            .as_str()
            .unwrap();
    let checkout = std::path::PathBuf::from(checkout);
    crate::common::commit(&checkout, "repaired.txt", "repair");
    git(&checkout, &["push", "-q", "origin", "HEAD"]);
    let branch = git(&checkout, &["symbolic-ref", "--short", "HEAD"]);
    world.pr(2000, repair, &branch);
    let fixed = world.afk_in(&checkout, &["land", "--issue", &repair.to_string()]).ok();
    assert_eq!(fixed["outcome"], "merged", "{fixed}");
    assert_eq!(fixed["recorded"], true);
    assert_eq!(world.state(repair), "closed");
    let normal = world.afk_in(&worker, &["land", "--issue", "1"]).ok();
    assert_eq!(normal["outcome"], "merged", "{normal}");
    assert!(worker_gated.exists());
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
}

#[test]
fn a_rejected_push_ends_the_turn_and_another_turn_gates_the_new_base() {
    let world = World::new();
    let started = world.root.join("started");
    let release = world.root.join("release");
    world.set_gate(&format!("if test -f worker.txt; then touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi", quote(&started), quote(&world.root), quote(&release)));
    seed(&world);
    let worker = world.worker(1, "worker.txt", "work");
    let child = start(&world, &worker, &world.cache(), 1);
    until(|| started.exists());
    crate::common::commit(&world.clone, "outside.txt", "another host");
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    let outside = world.remote(&["rev-parse", "refs/heads/main"]);
    std::fs::write(&release, "").unwrap();
    let rejected = finish(child);
    assert_eq!(rejected["outcome"], "target_moved", "{rejected}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), outside);
    assert_eq!(world.state(1), "open");
    let landed = world.afk_in(&worker, &["land", "--issue", "1"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(landed["merged"], outside);
    assert_eq!(landed["recorded"], true);
    assert_eq!(world.remote(&["show", "refs/heads/main:outside.txt"]), "another host");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
}

#[test]
fn an_old_stored_train_lanes_key_loads_and_a_run_drops_it() {
    let world = World::new();
    world.say_host_settings(json!({"train_lanes": 0, "gate_lanes": 2, "kept": 1}));
    {
        // A foreground hold reads the list and leaves it. Doctor still omits the key.
        let _runner = world.host_held();
        let doctor = world.afk(&["doctor"]).json;
        let settings = &doctor["host"]["settings"];
        assert_eq!(settings["in_force"]["gate_lanes"], 2);
        assert!(settings["said"].get("train_lanes").is_none(), "{settings}");
        assert!(settings["in_force"].get("train_lanes").is_none(), "{settings}");
        assert!(settings["in_force"].get("ignored").is_none(), "{settings}");
        assert!(doctor["host"]["exclusive"].get("train").is_none());
        assert_eq!(world.checkout_list()["settings"]["train_lanes"], 0);
    }
    let wrote = world.afk(&["run", "--admit-load", "400"]).ok();
    assert!(wrote["settings"].get("train_lanes").is_none(), "{wrote}");
    let file = world.checkout_list();
    assert!(file["settings"].get("train_lanes").is_none(), "{file}");
    assert_eq!(file["settings"]["gate_lanes"], 2);
    assert_eq!(file["settings"]["kept"], 1);
    assert_eq!(file["settings"]["admit_load"], 400);
}

#[test]
fn an_outside_red_base_is_certified_alone_and_its_fleet_repair_lands_in_a_turn() {
    red_base_repair(false, None);
}

#[test]
fn a_bound_repo_repairs_its_base_without_entering_its_human_workflow() {
    red_base_repair(true, None);
}

#[test]
fn an_old_repair_lock_at_a_bound_human_stage_becomes_an_operator_repair() {
    red_base_repair(true, Some("human"));
}

#[test]
fn an_old_repair_lock_bypasses_its_product_roles_unavailable_harness() {
    red_base_repair(true, Some("worker"));
}

/// A land that finds no open PR reads the remote once and writes nothing.
#[test]
fn a_land_that_finds_no_pr_reads_the_remote_once() {
    let world = World::new();
    world.watch_git();
    world.watch_pushes(None);
    let missing = "999999999";
    let expected = format!("no open PR closes issue #{missing} — if it has already merged, stop");
    for args in [vec!["land", "--issue", missing], vec!["land", missing]] {
        assert_eq!(world.afk(&args).error(), expected);
    }
    let git = world.git_calls();
    let listed: Vec<_> = git.iter().filter(|line| line.starts_with("ls-remote ")).collect();
    assert_eq!(listed.len(), 2, "{git:?}");
    for line in &listed {
        for name in
            ["refs/convoy/target/999999999", "refs/convoy/claim/999999999", "refs/heads/main"]
        {
            assert_eq!(line.matches(name).count(), 1, "{line}");
        }
    }
    let gh = world.gh_calls();
    let lists = gh.iter().filter(|call| call.windows(2).any(|pair| pair == ["pr", "list"])).count();
    let rates =
        gh.iter().filter(|call| call.windows(2).any(|pair| pair == ["api", "rate_limit"])).count();
    assert_eq!(lists, 2, "{gh:?}");
    assert_eq!(rates, 0, "{gh:?}");
    assert!(world.pushes().is_empty(), "{:?}", world.pushes());
    assert!(world.probes().is_empty(), "{:?}", world.probes());
}

/// A repair whose worker escalates waits for a human: later ticks, with the
/// tip still red, neither ready it again nor dispatch it (sunfmin/control#498).
#[test]
fn an_escalated_repair_stays_with_a_human_while_its_tip_stays_red() {
    let world = World::new();
    let tip = world.remote(&["rev-parse", "refs/heads/main"]);
    world.remote(&["update-ref", "refs/convoy/base/red/main", &tip]);
    let tick = |now: u64| {
        world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now.to_string()]).ok()
    };
    let first = tick(1000);
    let repair = world.gh()["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["title"].as_str().is_some_and(|s| s.starts_with("Repair the red")))
        .expect("the fleet opened a repair")["number"]
        .as_u64()
        .unwrap();
    let dispatched = |tick: &Value| {
        tick["did"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["do"] == "dispatch" && step["issue"] == repair)
    };
    assert!(dispatched(&first), "{first}");
    world.comment(repair, "<!--convoy:verdict phase=escalate-->\nThe gate's toolchain is missing.");
    world.with_gh(|gh| {
        gh["comments"][repair.to_string()].as_array_mut().unwrap().last_mut().unwrap()["at"] =
            json!(1050)
    });
    tick(1090);
    for now in [1180, 1270] {
        let later = tick(now);
        assert!(!dispatched(&later), "{later}");
        let labels = world.labels(repair);
        assert!(labels.contains(&"ready-for-human".into()), "{labels:?}");
        assert!(!labels.contains(&"ready-for-agent".into()), "{labels:?}");
        assert!(world.claims().is_empty());
    }
    let doctor = world.afk(&["doctor", "--now", "1300"]).json;
    let waits = doctor["waiting_on_human"].as_array().expect("doctor lists human waits");
    let wait = waits.iter().find(|wait| wait["issue"] == repair).expect("{doctor}");
    assert!(
        wait["answer"]
            .as_str()
            .unwrap()
            .contains("--add-label ready-for-agent --remove-label ready-for-human"),
        "{wait}"
    );
}

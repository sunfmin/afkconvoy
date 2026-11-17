//! A proving branch (docs/adr/0090): a fast-green round lands on it, and a
//! promotion moves the base branch once the slow gate passes there. Real
//! binary, real remotes; the gate script says which of its parts ran.
use crate::common::{World, commit, git};
use serde_json::json;
use std::path::{Path, PathBuf};

const PROVING: &str = "proving";

fn quote(path: &Path) -> String {
    afkconvoy_core::line::Line::new().word(&path.to_string_lossy()).own()
}

/// A world whose config splits its gate in two and names a proving branch,
/// which the remote holds at the base branch's tip. Each part of the gate
/// notes in the world's root that it ran, `ran-<part>`, and fails while
/// `fail-<part>` is there. PRs are opened against the proving branch.
fn proving_world() -> World {
    let world = World::new();
    let root = quote(&world.root);
    let script = format!(
        "#!/bin/sh\nset -e\nroot={root}\necho \"$(git rev-parse HEAD)\" >> \"$root/ran-$1\"\ntest ! -f \"$root/fail-$1\"\n"
    );
    commit(&world.clone, "gate.sh", &script);
    let afk = afkconvoy_core::line::Line::new().program(env!("CARGO_BIN_EXE_afkconvoy")).own();
    let config = format!(
        "```yaml\nbase_branch: main\nproving_branch: {PROVING}\ngate_command: \"{afk} exclusive -- sh ./gate.sh whole\"\ngate_seconds: 600\n```\n\n```yaml gates\nfast:\n  command: \"{afk} exclusive -- sh ./gate.sh fast\"\n  seconds: 300\n  budget_seconds: 200\nslow:\n  command: \"{afk} exclusive -- sh ./gate.sh slow\"\n  seconds: 300\n  budget_seconds: 200\n```\n"
    );
    std::fs::write(world.clone.join("docs/agents/afkconvoy.md"), config).unwrap();
    git(&world.clone, &["commit", "-q", "-am", "name a proving branch"]);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    git(&world.clone, &["push", "-q", "origin", &format!("HEAD:refs/heads/{PROVING}")]);
    world.with_gh(|gh| gh["base"] = json!(PROVING));
    world.cores_busy(10);
    world
}

/// A worker of issue `n` whose branch, cut from the proving branch, adds
/// `<name>.txt`, pushed, with its PR open against the proving branch.
fn worker(world: &World, n: u64, name: &str) -> PathBuf {
    let dir = world.root.join(format!("wt-{n}"));
    let branch = format!("issue-{n}");
    git(&world.clone, &["fetch", "-q", "origin"]);
    let at = dir.to_str().unwrap();
    let from = format!("origin/{PROVING}");
    git(&world.clone, &["worktree", "add", "-q", "-b", &branch, at, &from]);
    commit(&dir, &format!("{name}.txt"), name);
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.issue(n, &["ready-for-agent"]);
    world.pr(n * 10, n, &branch);
    dir
}

fn tip(world: &World, branch: &str) -> String {
    world.remote(&["rev-parse", &format!("refs/heads/{branch}")])
}

/// The commits the gate's `part` ran at, in the order it ran.
fn ran(world: &World, part: &str) -> Vec<String> {
    let kept = std::fs::read_to_string(world.root.join(format!("ran-{part}")));
    kept.unwrap_or_default().lines().map(str::to_string).collect()
}

/// Lands issues 1 and 2, each its own PR, on the proving branch → the base
/// branch's tip, which neither moved.
fn land_two(world: &World) -> String {
    let base = tip(world, "main");
    for (n, name) in [(1, "a"), (2, "b")] {
        let dir = worker(world, n, name);
        let landed = world.afk_in(&dir, &["land", "--issue", &n.to_string()]).ok();
        assert_eq!(landed["outcome"], "merged", "{landed}");
        assert_eq!(landed["target_branch"], PROVING, "{landed}");
        assert_eq!(tip(world, PROVING), landed["commit"].as_str().unwrap(), "{landed}");
        assert_eq!(world.state(n), "closed", "its issue closes as it lands on the proving branch");
    }
    assert_eq!(tip(world, "main"), base, "nothing lands on the base branch but by promotion");
    let files = world.remote(&["ls-tree", "--name-only", &format!("refs/heads/{PROVING}")]);
    assert!(files.lines().any(|f| f == "a.txt") && files.lines().any(|f| f == "b.txt"));
    assert!(ran(world, "slow").is_empty(), "no landing on the proving branch ran the slow gate");
    assert!(ran(world, "whole").is_empty(), "nor the whole gate");
    base
}

#[test]
fn two_fast_green_prs_land_on_the_proving_branch_and_a_promotion_moves_the_base_to_it() {
    let world = proving_world();
    let base = land_two(&world);
    let proving = tip(&world, PROVING);
    // plan and doctor say what waits for a promotion.
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["proving"]["proving_tip"], proving, "{}", plan["proving"]);
    assert_eq!(plan["proving"]["base_tip"], base);
    assert_eq!(plan["proving"]["unpromoted"], 2);
    assert!(plan["proving"]["oldest_waited_seconds"].is_u64(), "{}", plan["proving"]);
    let doctor = world.afk(&["doctor"]).ok();
    assert_eq!(doctor["fleet"]["proving"]["unpromoted"], 2, "{}", doctor["fleet"]);

    let promoted = world.afk(&["promote"]).ok();
    assert_eq!(promoted["outcome"], "promoted", "{promoted}");
    assert_eq!(promoted["gate"], "slow", "the fast run on record of the tip's tree is joined");
    assert_eq!(
        ran(&world, "slow"),
        std::slice::from_ref(&proving),
        "the slow gate ran once, on the tip"
    );
    assert!(ran(&world, "whole").is_empty());
    assert_eq!(tip(&world, "main"), proving, "the base branch fast-forwards to the tip");
    let again = world.afk(&["promote"]).ok();
    assert_eq!(again["outcome"], "current", "{again}");
    assert_eq!(world.afk(&["plan"]).ok()["proving"]["unpromoted"], 0);
}

#[test]
fn a_worker_started_after_a_landing_on_the_proving_branch_branches_from_it() {
    let world = proving_world();
    land_two(&world);
    let proving = tip(&world, PROVING);
    world.issue(3, &["ready-for-agent"]);
    world.kit_command("claude-code", "ckimi");
    let started = world
        .afk(&["dispatch", "--issue", "3", "--instance", "alpha", "--host", "h1", "--now", "1000"])
        .ok();
    let worktree = Path::new(started["worktree"].as_str().unwrap());
    assert_eq!(git(worktree, &["rev-parse", "HEAD"]), proving, "{started}");
    assert_eq!(world.afk(&["target", "--issue", "3"]).ok()["target_branch"], PROVING);
}

#[test]
fn a_promotion_whose_slow_gate_is_red_leaves_the_base_branch_and_says_so() {
    let world = proving_world();
    let base = land_two(&world);
    std::fs::write(world.root.join("fail-slow"), "").unwrap();
    let red = world.afk(&["promote"]).ok();
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert_eq!(tip(&world, "main"), base, "a red promotion moves nothing");
    let detail = red["detail"].as_str().unwrap();
    assert!(detail.contains(&format!("stays at {base}")), "{detail}");
    let runs = world.cache().join("afkconvoy").join("runs");
    let log: String = std::fs::read_dir(&runs)
        .unwrap()
        .map(|file| std::fs::read_to_string(file.unwrap().path()).unwrap())
        .collect();
    assert!(log.contains("promotion gate_red"), "its tick-log line says so: {log}");
}

#[test]
fn a_promotion_never_moves_the_base_branch_to_what_is_no_fast_forward_of_it() {
    let world = proving_world();
    land_two(&world);
    // The base branch moved past what the proving branch holds.
    let moved = world.land_elsewhere("elsewhere.txt");
    let answer = world.afk(&["promote"]).ok();
    assert_eq!(answer["outcome"], "diverged", "{answer}");
    assert_eq!(tip(&world, "main"), moved);
    assert!(ran(&world, "slow").is_empty(), "nothing is gated for a move it cannot make");
}

/// A command of issue `n` in the exclusive queue of `world`'s host, which
/// writes `$CACHE/held-<n>` once it runs and holds its place until the test
/// writes `$CACHE/release`.
fn held(world: &World, cache: &Path, n: u64) -> std::process::Child {
    let script = format!(
        "touch \"$CACHE/held-{n}\"; while test ! -f \"$CACHE/release\"; do sleep 0.02; done"
    );
    world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("XDG_CACHE_HOME", cache)
        .env("XDG_DATA_HOME", crate::common::data_beside(cache))
        .env("CACHE", cache)
        .env("AFKCONVOY_ISSUE", n.to_string())
        .args(["exclusive", "--", "sh", "-c", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// How many commands the exclusive queue of `world`'s host holds, running
/// or waiting, as `doctor` says it.
fn queued(world: &World) -> usize {
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let exclusive = &doctor["host"]["exclusive"];
    ["running", "waiting"].iter().map(|group| exclusive[group].as_array().map_or(0, Vec::len)).sum()
}

fn until(what: &str, done: impl Fn() -> bool) {
    afkconvoy_patience::until_true(what, done);
}

/// A tick of the fleet of `world`'s checkout, at the clock's time.
fn tick(world: &World) -> serde_json::Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1"]).ok()
}

#[test]
fn a_promotion_waits_while_a_worker_command_waits_and_starts_once_the_queue_empties() {
    let world = proving_world();
    let base = land_two(&world);
    let proving = tip(&world, PROVING);
    let _host_runner = world.host_held();
    let cache = world.cache();
    let running = held(&world, &cache, 11);
    until("the first command runs", || cache.join("held-11").exists());
    let waiting = held(&world, &cache, 12);
    until("the second command waits", || queued(&world) == 2);

    let ticked = tick(&world);
    let said = ticked["promotion"].as_str().unwrap_or_default();
    assert!(said.contains("host busy: 1 worker commands waiting"), "{ticked}");
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["proving"]["promotion"]["due"], "host_busy", "{}", plan["proving"]);
    let doctor = world.afk(&["doctor"]).ok();
    let line = doctor["fleet"]["proving"]["said"].as_str().unwrap_or_default();
    assert!(line.contains("host busy: 1 worker commands waiting"), "{}", doctor["fleet"]);
    assert_eq!(tip(&world, "main"), base, "no promotion starts while the host is busy");
    assert!(ran(&world, "slow").is_empty());

    std::fs::write(cache.join("release"), "").unwrap();
    assert!(running.wait_with_output().unwrap().status.success());
    assert!(waiting.wait_with_output().unwrap().status.success());
    until("the queue empties", || queued(&world) == 0);
    let ticked = tick(&world);
    assert!(ticked["promotion"].as_str().unwrap_or_default().contains("idle"), "{ticked}");
    until("the promotion moves the base branch", || tip(&world, "main") == proving);
    assert_eq!(ran(&world, "slow"), std::slice::from_ref(&proving));
}

#[test]
fn a_promotion_starts_at_the_bound_though_the_queue_never_empties() {
    let world = proving_world();
    land_two(&world);
    let proving = tip(&world, PROVING);
    world.say_host_settings(json!({"promotion_seconds": 1}));
    let _host_runner = world.host_held();
    let cache = world.cache();
    let running = held(&world, &cache, 11);
    until("the command runs", || cache.join("held-11").exists());
    // The oldest change landed a second or more before.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let ticked = tick(&world);
    let said = ticked["promotion"].as_str().unwrap_or_default();
    assert!(said.contains("starts anyway") && said.contains("host busy"), "{ticked}");
    // Its slow gate is queued behind the command, as any heavy command.
    until("the promotion's slow gate waits in the queue", || queued(&world) == 2);
    std::fs::write(cache.join("release"), "").unwrap();
    assert!(running.wait_with_output().unwrap().status.success());
    until("the promotion moves the base branch", || tip(&world, "main") == proving);
}

/// The repair issues of the proving branch the fake tracker holds, by number.
fn repairs(world: &World) -> Vec<serde_json::Value> {
    let gh = world.gh();
    let rows = gh["issues"].as_array().unwrap().clone();
    rows.into_iter()
        .filter(|i| i["title"].as_str().unwrap_or("").starts_with("Repair the red promotion"))
        .collect()
}

#[test]
fn a_red_promotion_gets_one_repair_issue_and_the_next_green_promotion_closes_it() {
    let world = proving_world();
    land_two(&world);
    std::fs::write(world.root.join("fail-slow"), "").unwrap();
    let red = world.afk(&["promote"]).ok();
    assert_eq!(red["outcome"], "gate_red", "{red}");
    let found = repairs(&world);
    assert_eq!(found.len(), 1, "one red promotion has one repair issue: {found:?}");
    let issue = found[0]["number"].as_u64().unwrap();
    assert_eq!(red["repair_issue"], issue);
    let labels: Vec<&str> =
        found[0]["labels"].as_array().unwrap().iter().filter_map(|l| l.as_str()).collect();
    assert!(labels.contains(&"convoy-repair") && labels.contains(&"ready-for-agent"), "{labels:?}");
    assert!(!labels.contains(&"fleet-lock"), "a red proving branch holds no ordinary turn");
    let body = found[0]["body"].as_str().unwrap();
    assert!(body.contains("Failing tests") && body.contains("Gate output"), "{body}");
    assert!(body.contains("#1, PR #") && body.contains("#2, PR #"), "{body}");

    // A second red while it is open adds to it and opens no other.
    let again = world.afk(&["promote"]).ok();
    assert_eq!(again["repair_issue"], issue, "{again}");
    assert_eq!(repairs(&world).len(), 1);
    assert!(
        world.comments(issue).iter().any(|c| c.contains("the promotion is red again")),
        "{:?}",
        world.comments(issue)
    );

    // A fast-green PR lands on the red proving branch, which holds nothing back.
    let dir = worker(&world, 50, "c");
    let landed = world.afk_in(&dir, &["land", "--issue", "50"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");

    // The fix lands; the next promotion is green and closes the issue.
    std::fs::remove_file(world.root.join("fail-slow")).unwrap();
    assert_eq!(world.state(issue), "open");
    let promoted = world.afk(&["promote"]).ok();
    assert_eq!(promoted["outcome"], "promoted", "{promoted}");
    assert_eq!(promoted["repair_closed"], issue);
    assert_eq!(world.state(issue), "closed");
}

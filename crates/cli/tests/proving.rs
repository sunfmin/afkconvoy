//! A proving branch (docs/adr/0090): a fast-green round lands on it, and a
//! promotion moves the base branch once the slow gate passes there. Real
//! binary, real remotes; the gate script says which of its parts ran.
use crate::common::{World, commit, data_beside, git};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

const PROVING: &str = "proving";
/// The two hosts of a world: the world's own, and another (`Host`).
const A: &str = "host-a";
const B: &str = "host-b";

fn quote(path: &Path) -> String {
    afkconvoy_core::line::Line::new().word(&path.to_string_lossy()).own()
}

/// A world whose config splits its gate in two and names a proving branch,
/// which the remote holds at the base branch's tip. Each part of the gate
/// notes in the world's root that it ran, `ran-<part>`, and fails while
/// `fail-<part>` is there. It waits while a file `hold-<part>…` of the root
/// is there whose words, run in the gate's worktree, succeed, saying so by
/// `<that file>.held`. PRs are opened against the proving branch.
fn proving_world() -> World {
    let world = World::new();
    let root = quote(&world.root);
    let script = format!(
        "#!/bin/sh\nset -e\nroot={root}\necho \"$(git rev-parse HEAD)\" >> \"$root/ran-$1\"\nfor hold in \"$root\"/hold-$1*; do\n  case \"$hold\" in *.held) continue;; esac\n  test -f \"$hold\" || continue\n  if sh -c \"$(cat \"$hold\")\"; then touch \"$hold.held\"; while test -f \"$hold\"; do sleep 0.05; done; fi\ndone\ntest ! -f \"$root/fail-$1\"\n"
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
    worker_in(world, &world.clone, n, name)
}

/// `worker`, in a worktree of the clone `clone`: a host's own.
fn worker_in(world: &World, clone: &Path, n: u64, name: &str) -> PathBuf {
    let dir = world.root.join(format!("wt-{n}"));
    let branch = format!("issue-{n}");
    git(clone, &["fetch", "-q", "origin"]);
    let at = dir.to_str().unwrap();
    let from = format!("origin/{PROVING}");
    git(clone, &["worktree", "add", "-q", "-b", &branch, at, &from]);
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
        let landed = world.afk_in(&dir, &["land", "--issue", &n.to_string(), "--host", A]).ok();
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

/// Another host of the world's repo (sunfmin/control#1334): a clone of the
/// remote of its own, and a cache and data directory of its own, and so its
/// own host database, candidate chains and exclusive queue. GitHub's
/// stand-in and the remote are the world's.
struct Host {
    clone: PathBuf,
    cache: PathBuf,
}

impl Host {
    fn new(world: &World, name: &str) -> Host {
        let clone = world.clone_again(&format!("{name}-clone"));
        let cache = world.root.join(format!("{name}-cache"));
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(data_beside(&cache)).unwrap();
        Host { clone, cache }
    }

    fn command(&self, world: &World, cwd: &Path, args: &[&str]) -> std::process::Command {
        let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), cwd);
        command
            .env("XDG_CACHE_HOME", &self.cache)
            .env("XDG_DATA_HOME", data_beside(&self.cache))
            .args(args);
        command
    }

    /// The CLI run on this host, in `cwd` → its one JSON answer.
    fn afk(&self, world: &World, cwd: &Path, args: &[&str]) -> Value {
        let out = self.command(world, cwd, args).output().unwrap();
        answer(&out.stdout, &out.stderr)
    }

    /// The CLI started on this host, in `cwd`, and left running.
    fn start(&self, world: &World, cwd: &Path, args: &[&str]) -> Running {
        let child = self
            .command(world, cwd, args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Running(Some(child))
    }

    /// A stand-in host runner of this host, on its socket: it puts a
    /// command that asks for a lane in the exclusive queue behind one other,
    /// tells `asked` the question, and gives it its lane once `go` is sent.
    fn runner(&self) -> Runner {
        let dir = self.cache.join("afkconvoy");
        std::fs::create_dir_all(&dir).unwrap();
        let short = tempfile::tempdir_in("/tmp").unwrap();
        let link = short.path().join("c");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let listener = UnixListener::bind(link.join("host-runner.sock")).unwrap();
        let (tell, asked) = channel();
        let (go, given) = channel::<()>();
        let given = Arc::new(Mutex::new(given));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let tell: Sender<Value> = tell.clone();
                let given = given.clone();
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let question: Value = serde_json::from_str(line.trim()).unwrap_or_default();
                    if question["op"] != "exclusive" {
                        return;
                    }
                    let runner = json!({"pid": 4242, "version": "test", "log": "", "panel": null});
                    let _ = writeln!(stream, "{}", json!({"ahead": 1, "host_runner": runner}));
                    let _ = tell.send(question);
                    let _ = given.lock().unwrap().recv();
                    let _ = writeln!(stream, "{}", json!({"runs": 1, "ahead_of": 0}));
                    // Its lane is held until the asker closes its end.
                    let _ = std::io::copy(&mut reader, &mut std::io::sink());
                });
            }
        });
        Runner { asked, go, _short: short }
    }

    /// The fleet's log of this host.
    fn log(&self) -> String {
        let runs = self.cache.join("afkconvoy").join("runs");
        std::fs::read_dir(&runs)
            .map(|files| {
                files.map(|file| std::fs::read_to_string(file.unwrap().path()).unwrap()).collect()
            })
            .unwrap_or_default()
    }
}

/// A host's stand-in host runner (`Host::runner`).
struct Runner {
    asked: Receiver<Value>,
    go: Sender<()>,
    _short: tempfile::TempDir,
}

/// A command left running; killed where the test ends before it.
struct Running(Option<Child>);

impl Running {
    /// Whether it ended.
    fn ended(&mut self) -> bool {
        self.0.as_mut().unwrap().try_wait().unwrap().is_some()
    }

    /// Its one JSON answer, once it ended.
    fn answer(mut self) -> Value {
        let mut child = self.0.take().unwrap();
        let (mut stdout, mut stderr) = (vec![], vec![]);
        child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
        child.stderr.take().unwrap().read_to_end(&mut stderr).unwrap();
        child.wait().unwrap();
        answer(&stdout, &stderr)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn answer(stdout: &[u8], stderr: &[u8]) -> Value {
    let stdout = String::from_utf8_lossy(stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("not one JSON object ({e}): {stdout:?}\nstderr: {}", String::from_utf8_lossy(stderr))
    })
}

/// Holds the gate's `part` while `words`, run in its worktree, succeed, as
/// `hold` names it → what to remove to let it go, once it holds.
fn hold(world: &World, running: &mut Running, part: &str, name: &str, words: &str) -> PathBuf {
    let file = world.root.join(format!("hold-{part}-{name}"));
    std::fs::write(&file, words).unwrap();
    let held = world.root.join(format!("hold-{part}-{name}.held"));
    afkconvoy_patience::until_true(format!("the {part} gate held for {name}"), || {
        assert!(!running.ended(), "it ended before its {part} gate held");
        held.exists()
    });
    file
}

/// The words a hold runs: whether the gate runs in a checkout of `clone`.
fn in_clone(clone: &Path) -> String {
    let common = std::fs::canonicalize(clone.join(".git")).unwrap();
    format!("test \"$(cd \"$(git rev-parse --git-common-dir)\" && pwd -P)\" = {}", quote(&common))
}

#[test]
fn two_hosts_land_on_one_proving_branch_and_the_refused_one_is_built_again_on_the_new_tip() {
    let world = proving_world();
    let b = Host::new(&world, B);
    let base = tip(&world, "main");
    let first = worker(&world, 1, "a");
    let second = worker_in(&world, &b.clone, 2, "b");
    // Host B's round is fast-gating on the proving tip it read...
    let mut turn = b.start(&world, &second, &["land", "--issue", "2", "--host", B]);
    let held = hold(&world, &mut turn, "fast", "b", "test -f b.txt");
    // ...when host A lands its own on the same proving branch.
    let landed = world.afk_in(&first, &["land", "--issue", "1", "--host", A]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let a_tip = tip(&world, PROVING);
    assert_eq!(landed["commit"], a_tip.as_str(), "{landed}");
    std::fs::remove_file(held).unwrap();
    let refused = turn.answer();
    assert_eq!(refused["outcome"], "target_moved", "its push required the tip it read: {refused}");
    assert_eq!(tip(&world, PROVING), a_tip, "the refused round landed nothing");
    assert_eq!(world.state(2), "open");
    // Its next turn reads the tip again, builds on it and gates that.
    let again = b.afk(&world, &second, &["land", "--issue", "2", "--host", B]);
    assert_eq!(again["outcome"], "merged", "{again}");
    let b_tip = tip(&world, PROVING);
    assert_eq!(again["commit"], b_tip.as_str(), "{again}");
    assert_eq!(world.remote(&["rev-parse", &format!("{b_tip}^1")]), a_tip, "built on A's");
    let files = world.remote(&["ls-tree", "--name-only", &format!("refs/heads/{PROVING}")]);
    assert!(files.lines().any(|f| f == "a.txt") && files.lines().any(|f| f == "b.txt"));
    // Neither lands an untested tree: each tip the proving branch took is a
    // commit its fast gate ran on.
    let fast = ran(&world, "fast");
    assert!(fast.contains(&a_tip) && fast.contains(&b_tip), "{fast:?}");
    assert_eq!(tip(&world, "main"), base, "nothing lands on the base branch but by promotion");
    // Each landing names its host, and a promotion says whose it covers.
    let promoted = world.afk(&["promote", "--host", A]).ok();
    assert_eq!(promoted["outcome"], "promoted", "{promoted}");
    assert_eq!(promoted["landed_on"], json!({A: 1, B: 1}), "{promoted}");
}

#[test]
fn a_promotion_gates_what_another_host_landed_while_it_waited_for_its_lane() {
    let world = proving_world();
    let base = land_two(&world);
    let b = Host::new(&world, B);
    let runner = b.runner();
    let promotion = b.start(&world, &b.clone, &["promote", "--host", B]);
    // B's promotion read the proving tip and is queued for its lane...
    let asked = runner.asked.recv_timeout(crate::common::PATIENCE).expect("it asked for a lane");
    assert_eq!(asked["ticket"]["command"], "promote: the proving branch's tip", "{asked}");
    let before = tip(&world, PROVING);
    // ...when host A lands on the proving branch.
    let dir = worker(&world, 3, "c");
    let landed = world.afk_in(&dir, &["land", "--issue", "3", "--host", A]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let after = tip(&world, PROVING);
    assert_ne!(after, before);
    runner.go.send(()).unwrap();
    let promoted = promotion.answer();
    assert_eq!(promoted["outcome"], "promoted", "{promoted}");
    assert_eq!(promoted["proving_tip"], after.as_str(), "it read the tip again: {promoted}");
    assert_eq!(promoted["gated_tip"], after.as_str(), "{promoted}");
    // The fast run of that tip is host A's record, not B's (docs/adr/0037):
    // B gates it whole, once.
    assert_eq!(ran(&world, "whole"), std::slice::from_ref(&after), "one run, of that tip");
    assert!(ran(&world, "slow").is_empty(), "{:?}", ran(&world, "slow"));
    assert_eq!(tip(&world, "main"), after, "the base branch fast-forwards to it");
    assert_eq!(promoted["base_tip"], base.as_str(), "{promoted}");
    assert_eq!(promoted["unpromoted"], 3, "{promoted}");
    assert_eq!(promoted["landed_on"], json!({A: 3}), "{promoted}");
    let log = b.log();
    let line = log.lines().find(|line| line.contains("promotion promoted")).unwrap_or_else(|| {
        panic!("its tick-log line says it: {log}");
    });
    assert!(line.contains(&format!("proving tip {after}")), "{line}");
    assert!(line.contains("3 landing(s), from host-a 3"), "{line}");
}

#[test]
fn a_second_host_s_promotion_of_a_tip_the_base_already_holds_pushes_nothing() {
    let world = proving_world();
    land_two(&world);
    let proving = tip(&world, PROVING);
    let b = Host::new(&world, B);
    let mut late = b.start(&world, &b.clone, &["promote", "--host", B]);
    // B has no fast run of the tip on its record: it gates it whole.
    let held = hold(&world, &mut late, "whole", B, &in_clone(&b.clone));
    let first = world.afk(&["promote", "--host", A]).ok();
    assert_eq!(first["outcome"], "promoted", "{first}");
    assert_eq!(tip(&world, "main"), proving);
    world.watch_pushes(None);
    std::fs::remove_file(held).unwrap();
    let second = late.answer();
    assert_eq!(second["outcome"], "already_promoted", "{second}");
    assert_eq!(second["pushed"], false, "{second}");
    assert_eq!(second["gated_tip"], proving.as_str(), "{second}");
    assert!(world.pushes().iter().all(|r| !r.contains("refs/heads/main")), "{:?}", world.pushes());
    assert_eq!(tip(&world, "main"), proving);
}

#[test]
fn a_second_host_s_promotion_of_a_tip_ahead_of_the_first_s_fast_forwards_onto_it() {
    let world = proving_world();
    let base = tip(&world, "main");
    let b = Host::new(&world, B);
    let one = worker(&world, 1, "a");
    assert_eq!(
        world.afk_in(&one, &["land", "--issue", "1", "--host", A]).ok()["outcome"],
        "merged"
    );
    let older = tip(&world, PROVING);
    // A's promotion gates the older tip...
    let mut early = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    early
        .args(["promote", "--host", A])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut early = Running(Some(early.spawn().unwrap()));
    let held_a = hold(&world, &mut early, "slow", A, &in_clone(&world.clone));
    let two = worker(&world, 2, "b");
    assert_eq!(
        world.afk_in(&two, &["land", "--issue", "2", "--host", A]).ok()["outcome"],
        "merged"
    );
    let newer = tip(&world, PROVING);
    // ...and B's the newer one, read while the base branch was at neither.
    let mut late = b.start(&world, &b.clone, &["promote", "--host", B]);
    // B has no fast run of the tip on its record: it gates it whole.
    let held_b = hold(&world, &mut late, "whole", B, &in_clone(&b.clone));
    std::fs::remove_file(held_a).unwrap();
    let first = early.answer();
    assert_eq!(first["outcome"], "promoted", "{first}");
    assert_eq!(first["gated_tip"], older.as_str());
    assert_eq!(tip(&world, "main"), older);
    std::fs::remove_file(held_b).unwrap();
    let second = late.answer();
    assert_eq!(second["outcome"], "promoted", "{second}");
    assert_eq!(second["pushed"], true, "{second}");
    assert_eq!(second["gated_tip"], newer.as_str(), "{second}");
    assert_eq!(second["base_tip"], older.as_str(), "it fast-forwarded from the first's: {second}");
    assert_eq!(tip(&world, "main"), newer);
    assert_ne!(older, base);
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

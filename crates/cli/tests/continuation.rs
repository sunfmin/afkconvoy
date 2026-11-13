use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{World, commit, git};
use serde_json::{Value, json};

const BRANCH: &str = "tester/issue-7-issue-7";

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world with a worker dispatched on issue 7 at 1000.
fn fleet() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done", "heartbeat done"]);
    world
}

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == 7).expect("a worktree of issue 7");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The worker's terminal dies; its worktree stays.
fn kill_worker(world: &World) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
}

/// The worktree is gone from this host, as on another machine.
fn lose_worktree(world: &World) {
    world.with_orca(|orca| {
        orca["worktrees"] = json!([]);
        orca["terminals"] = json!([]);
    });
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

fn continues(prompt: &str) -> bool {
    prompt.starts_with("You are an afkconvoy worker **continuing**")
}

fn open_terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).cloned().collect()
}

fn recovery(world: &World) -> Value {
    world.afk(&["recovery", "--issue", "7", "--now", "1100"]).ok()
}

fn dispatch(world: &World, more: &[&str]) -> common::Answer {
    let mut args = vec!["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1"];
    args.extend(["--now", "1100"]);
    args.extend(more);
    world.afk(&args)
}

/// A real helper left running after Orca's terminal is gone. A separate
/// thread reaps it as soon as it exits, including while the CLI is running.
struct Escaped {
    pid: u32,
    reaper: Option<std::thread::JoinHandle<std::process::ExitStatus>>,
}

impl Escaped {
    fn start(world: &World, dir: &Path, ignores_term: bool) -> Self {
        Self::in_directory(world, dir, dir, ignores_term)
    }

    fn in_directory(world: &World, worktree: &Path, dir: &Path, ignores_term: bool) -> Self {
        let ready = dir.join("escaped-ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "continuation::escaped_helper", "--ignored"])
            .current_dir(dir)
            .env("TMPDIR", world.tmp(worktree))
            .env("ESCAPED_READY", &ready)
            .env("ESCAPED_IGNORES_TERM", ignores_term.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let escaped = Self { pid, reaper: Some(std::thread::spawn(move || child.wait().unwrap())) };
        let until = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < until, "escaped helper never became ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        escaped
    }

    fn ended(&mut self) -> std::process::ExitStatus {
        assert!(self.reaper.as_ref().unwrap().is_finished(), "helper {} still runs", self.pid);
        self.reaper.take().unwrap().join().unwrap()
    }
}

impl Drop for Escaped {
    fn drop(&mut self) {
        if let Some(reaper) = self.reaper.take() {
            // SAFETY: this pid belongs to the test's helper, which this
            // guard owns until the reaper has observed its exit.
            if !reaper.is_finished() {
                unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
            }
            reaper.join().unwrap();
        }
    }
}

#[test]
#[ignore = "the continuation tests start this as an escaped worker process"]
fn escaped_helper() {
    if std::env::var("ESCAPED_IGNORES_TERM").as_deref() == Ok("true") {
        // SAFETY: this isolated helper deliberately ignores SIGTERM.
        unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
    }
    std::fs::write(std::env::var_os("ESCAPED_READY").unwrap(), std::process::id().to_string())
        .unwrap();
    std::thread::sleep(Duration::from_secs(120));
}

#[test]
#[ignore = "the orphan regression starts this as the helper's short-lived parent"]
#[allow(clippy::zombie_processes, reason = "the regression requires this parent to exit first")]
fn orphan_launcher() {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "continuation::escaped_helper", "--ignored"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
}

/// A helper whose parent really exited: the same ppid=1 distinction that
/// separates a worker's leftovers from a server shared by the host.
struct Orphan(u32);

impl Orphan {
    fn start(world: &World, worktree: &Path, cwd: &Path) -> Self {
        let ready = cwd.join("orphan-ready");
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "continuation::orphan_launcher", "--ignored"])
            .current_dir(cwd)
            .env("TMPDIR", world.tmp(worktree))
            .env("ESCAPED_READY", &ready)
            .env("ESCAPED_IGNORES_TERM", "false")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(ready) = std::fs::read_to_string(&ready) {
                let pid: u32 = ready.parse().unwrap();
                let ppid = Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "ppid="])
                    .output()
                    .unwrap();
                assert_eq!(String::from_utf8_lossy(&ppid.stdout).trim(), "1");
                return Self(pid);
            }
            assert!(Instant::now() < until, "orphan never became ready");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn alive(&self) -> bool {
        // SAFETY: signal zero observes only, without delivering a signal.
        unsafe { libc::kill(self.0 as libc::pid_t, 0) == 0 }
    }
}

impl Drop for Orphan {
    fn drop(&mut self) {
        if self.alive() {
            // SAFETY: this orphan was started and is owned by this test.
            unsafe { libc::kill(self.0 as libc::pid_t, libc::SIGKILL) };
        }
    }
}

#[test]
fn an_orphan_in_the_worktree_ends_while_a_shared_server_with_its_tmpdir_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let world = fleet();
    let dir = worktree(&world);
    let escaped = Orphan::start(&world, &dir, &dir);
    let server = Orphan::start(&world, &dir, &world.root);
    let kill = world.root.join("bin/kill");
    std::fs::write(
        &kill,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${0%/*}/../signals\"\nexec /bin/kill \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&kill, std::fs::Permissions::from_mode(0o755)).unwrap();
    kill_worker(&world);

    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked)[0], "judge 7 continued", "{ticked}");
    assert!(!escaped.alive(), "orphan in the worktree still runs");
    assert!(server.alive(), "server shared by the host was ended");
    let signals = std::fs::read_to_string(world.root.join("signals")).unwrap();
    assert_eq!(signals.trim(), format!("-TERM {}", escaped.0));
    assert_eq!(open_terminals(&world, &dir).len(), 1);
}

#[test]
fn an_escaped_worker_process_is_ended_before_the_continuation_starts() {
    ends_escaped(false, libc::SIGTERM);
}

#[test]
fn an_escaped_worker_ignoring_term_is_forced_to_end_before_the_continuation_starts() {
    ends_escaped(true, libc::SIGKILL);
}

fn entries(world: &World) -> Vec<Value> {
    std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .flatten()
        .filter(|file| file.path().extension().is_some_and(|ext| ext == "jsonl"))
        .flat_map(|file| {
            std::fs::read_to_string(file.path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn ends_escaped(ignores_term: bool, signal: i32) {
    use std::os::unix::process::ExitStatusExt;
    let world = fleet();
    let dir = worktree(&world);
    let mut escaped = Escaped::start(&world, &dir, ignores_term);
    let other_dir = world.root.join("outside");
    std::fs::create_dir(&other_dir).unwrap();
    // An unrelated helper, with no TMPDIR belonging to this worktree.
    let outside = Escaped::start(&world, &other_dir, false);
    kill_worker(&world);

    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked)[0], "judge 7 continued", "{ticked}");
    assert_eq!(escaped.ended().signal(), Some(signal));
    assert!(!outside.reaper.as_ref().unwrap().is_finished(), "outside process was signalled");
    assert_eq!(open_terminals(&world, &dir).len(), 1);
    let logged = entries(&world);
    let lines: Vec<_> = logged
        .iter()
        .filter(|entry| entry["command"]["command"] == "start")
        .map(|entry| entry["command"]["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{logged:?}");
    assert!(lines[0].contains(&format!("pid {}", escaped.pid)), "{lines:?}");
    assert!(lines[0].contains("continuation::escaped_helper"), "{lines:?}");
    assert!(lines[0].contains("ran "), "{lines:?}");
    assert!(!lines[0].contains("ESCAPED_READY="), "never log the environment: {lines:?}");
}

#[test]
fn a_process_that_survives_kill_is_named_when_the_repeated_start_escalates() {
    use std::os::unix::fs::PermissionsExt;
    let world = fleet();
    let dir = worktree(&world);
    let mut escaped = Escaped::start(&world, &dir, false);
    kill_worker(&world);
    // Simulate an unkillable process: record each signal without delivering
    // it. The process and all runtime observations are otherwise real.
    let kill = world.root.join("bin/kill");
    std::fs::write(&kill, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${0%/*}/../signals\"\n").unwrap();
    std::fs::set_permissions(&kill, std::fs::Permissions::from_mode(0o755)).unwrap();

    let first = tick(&world, 1090);
    assert_eq!(did(&first)[0], "judge 7 failed", "{first}");
    let second = tick(&world, 1180);
    assert_eq!(did(&second)[0], "judge 7 failed", "{second}");
    assert!(second["errors"][0]["error"].as_str().unwrap().contains("the claim was escalated"));
    assert!(world.labels(7).iter().any(|label| label == "ready-for-human"));
    let comments = world.comments(7).join("\n");
    assert!(comments.contains("survived SIGKILL"), "{comments}");
    assert!(comments.contains(&format!("pid {}", escaped.pid)), "{comments}");
    assert!(comments.contains("continuation::escaped_helper"), "{comments}");
    assert_eq!(open_terminals(&world, &dir).len(), 0);
    let signals = std::fs::read_to_string(world.root.join("signals")).unwrap();
    assert_eq!(
        signals.lines().collect::<Vec<_>>(),
        vec![
            format!("-TERM {}", escaped.pid),
            format!("-KILL {}", escaped.pid),
            format!("-TERM {}", escaped.pid),
            format!("-KILL {}", escaped.pid),
        ]
    );
    // The helper really remains alive until the guard ends it.
    assert!(!escaped.reaper.as_mut().unwrap().is_finished());
}

#[test]
fn a_claim_whose_worker_died_is_continued_in_the_worktree_still_here() {
    let world = fleet();
    world.list_checkout(&world.clone);
    world.issue(8, &["ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1090))[1], "dispatch 8 done");
    // an issue comes to the frontier as the worker of #7 dies
    world.issue(9, &["ready-for-agent"]);
    let dir = worktree_of(&world, 7);
    commit(&dir, "work.txt", "x\n");
    std::fs::write(dir.join("scratch.txt"), "uncommitted").unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == dir.to_str().unwrap() {
                terminal["open"] = json!(false);
            }
        }
    });
    world.orca_calls();
    world.watch_pushes(None);

    let ticked = tick(&world, 1180);
    let did = did(&ticked);
    assert_eq!(did[0], "judge 7 continued");
    assert!(
        did.contains(&"dispatch 9 done".to_string()),
        "the frontier waits for nothing: {did:?}"
    );
    assert_eq!(ticked["held"], 3);
    let claims: Vec<u64> = world.claims().iter().map(|claim| claim.0).collect();
    assert_eq!(claims, [7, 8, 9], "#7 never released back to the frontier");
    assert!(!world.pushes().iter().any(|push| push.contains("claim/7")), "nor pushed again");

    // the same worktree, with everything in it, and a new worker told to inspect it first
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.contains(&"worktree create".to_string()), "{calls:?}");
    assert!(!calls.contains(&"worktree rm".to_string()), "{calls:?}");
    assert_eq!(std::fs::read_to_string(dir.join("scratch.txt")).unwrap(), "uncommitted");
    assert_eq!(open_terminals(&world, &dir).len(), 1);
    assert!(continues(&brief(&dir)));
    assert!(brief(&dir).contains(&format!("**Your branch:** `{BRANCH}`")));
    let comments = world.comments(7);
    assert_eq!(
        comments.iter().filter(|body| body.contains("<!--convoy:branch ")).count(),
        1,
        "its branch was recorded already: {comments:?}"
    );
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:worker ")).count(),
        1,
        "a continuation of the same harness is not another row: {comments:?}"
    );
}

fn worktree_of(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == issue).expect("a worktree of the issue");
    PathBuf::from(row["path"].as_str().unwrap())
}

#[test]
fn a_pristine_worktree_is_reused_with_nothing_said_to_be_there() {
    let world = fleet();
    kill_worker(&world);
    let found = recovery(&world);
    assert_eq!((found["tier"].clone(), found["prompt"].clone()), (json!(1), json!("fresh")));
    assert_eq!(did(&tick(&world, 1090))[0], "judge 7 continued");
    assert!(!continues(&brief(&worktree(&world))));
}

#[test]
fn with_no_worktree_here_the_pushed_branch_is_recreated_at_its_tip() {
    let world = fleet();
    let old = worktree(&world);
    let pushed = commit(&old, "work.txt", "x\n");
    git(&old, &["push", "-q", "origin", "HEAD"]);
    lose_worktree(&world);

    let found = recovery(&world);
    assert_eq!(found["tier"], 2, "{found}");
    assert_eq!(found["action"], "recreate_at_tip");
    assert_eq!(found["worktree"]["present"], false);
    assert_eq!(
        found["branch"],
        json!({"name": BRANCH, "candidates": [BRANCH], "commits_ahead": 1})
    );

    assert_eq!(did(&tick(&world, 1090))[0], "judge 7 continued");
    let new = worktree(&world);
    assert_ne!(new, old);
    assert_eq!(git(&new, &["rev-parse", "HEAD"]), pushed, "cut at the tip that was pushed");
    assert!(continues(&brief(&new)));
    assert_eq!(open_terminals(&world, &new).len(), 1);
    // nothing was torn down: the branch it continues from is still the remote's
    assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{BRANCH}")]), pushed);
    // …and the branch orca cut for the new worktree is the issue's own too
    let comments = world.comments(7);
    assert!(
        comments
            .iter()
            .any(|body| body.starts_with(&format!("<!--convoy:branch name={BRANCH}-2-->"))),
        "{comments:?}"
    );
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:branch ")).count(),
        2,
        "{comments:?}"
    );
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:worker ")).count(),
        1,
        "recreating the worktree continues the same worker: {comments:?}"
    );
    assert!(
        !comments.iter().any(|body| body.starts_with("<!--convoy:account ")),
        "a continuation is not an end: {comments:?}"
    );
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn with_nothing_survived_the_start_is_fresh_from_the_base_branch() {
    let world = fleet();
    let old = worktree(&world);
    // committed, never pushed: gone with the worktree
    commit(&old, "work.txt", "x\n");
    // a branch that only looks like the issue's is somebody else's
    git(
        &world.clone,
        &["push", "-q", "origin", &format!("{BRANCH}:refs/heads/tester/issue-7-lookalike")],
    );
    lose_worktree(&world);

    let found = recovery(&world);
    assert_eq!(
        (found["tier"].clone(), found["action"].clone()),
        (json!(3), json!("dispatch_fresh"))
    );
    assert_eq!(found["branch"]["candidates"], json!([]), "nothing is read off a branch's name");

    assert_eq!(did(&tick(&world, 1090))[0], "judge 7 continued");
    let new = worktree(&world);
    assert_eq!(git(&new, &["rev-parse", "HEAD"]), world.remote(&["rev-parse", "refs/heads/main"]));
    assert!(!continues(&brief(&new)));
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn of_several_own_branches_the_one_furthest_ahead_is_continued() {
    let world = fleet();
    let first = worktree(&world);
    commit(&first, "a.txt", "x\n");
    git(&first, &["push", "-q", "origin", "HEAD"]);
    lose_worktree(&world);
    assert_eq!(did(&tick(&world, 1090))[0], "judge 7 continued");
    // the continuation went further, on the branch orca cut for it
    let second = worktree(&world);
    let further = commit(&second, "b.txt", "x\n");
    git(&second, &["push", "-q", "origin", "HEAD"]);
    lose_worktree(&world);

    let found = recovery(&world);
    let later = format!("{BRANCH}-2");
    assert_eq!(found["branch"]["candidates"], json!([BRANCH, later]));
    assert_eq!(found["branch"]["name"], json!(later));
    assert_eq!(found["branch"]["commits_ahead"], 2);
    assert_eq!(did(&tick(&world, 1180))[0], "judge 7 continued");
    assert_eq!(git(&worktree(&world), &["rev-parse", "HEAD"]), further);
}

#[test]
fn recovery_shows_the_tier_and_changes_nothing() {
    let world = fleet();
    let dir = worktree(&world);
    commit(&dir, "work.txt", "x\n");
    kill_worker(&world);
    world.orca_calls();
    world.gh_calls();
    world.watch_pushes(None);

    let found = recovery(&world);
    assert_eq!(found["issue"], 7);
    assert_eq!(found["base"], "main");
    assert_eq!(
        (found["tier"].clone(), found["action"].clone()),
        (json!(1), json!("reuse_worktree"))
    );
    assert_eq!(found["prompt"], "continue");
    assert_eq!(found["reason"], "local worktree present with 1 commit(s) ahead");
    let here = json!({"present": true, "path": dir, "commits_ahead": 1, "dirty": false});
    assert_eq!(found["worktree"], here);

    assert_eq!(world.pushes(), [] as [String; 0]);
    let orca: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert_eq!(orca, ["repo list", "worktree list"]);
    let wrote = world.gh_calls().iter().any(|call| call.contains(&"--method".to_string()));
    assert!(!wrote);
    assert_eq!(open_terminals(&world, &dir).len(), 0);
}

#[test]
fn a_remote_that_cannot_be_read_is_an_error_never_nothing_survived() {
    let world = fleet();
    lose_worktree(&world);
    let bare = world.bare.to_str().unwrap().to_string();
    git(&world.clone, &["remote", "set-url", "origin", "/nowhere/at/all.git"]);
    let error = world.afk(&["recovery", "--issue", "7", "--now", "1100"]).error();
    assert!(error.contains("ls-remote") || error.contains("git"), "{error}");
    git(&world.clone, &["remote", "set-url", "origin", &bare]);

    // a continuation that fails leaves the claim held, and the next tick makes it again
    world.with_orca(|orca| orca["create_fails"] = json!([7]));
    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert_eq!(world.claims().len(), 1);
    world.with_orca(|orca| orca["create_fails"] = json!([]));
    assert_eq!(did(&tick(&world, 1180))[0], "judge 7 continued");
}

#[test]
fn start_fresh_is_the_one_way_to_discard_an_attempt() {
    let world = fleet();
    let old = worktree(&world);
    let pushed = commit(&old, "work.txt", "x\n");
    git(&old, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, BRANCH);
    // somebody else's PR for the issue, and a branch that only has the name
    world.pr(71, 7, "someone/else");
    let lookalike = "refs/heads/tester/issue-7-lookalike";
    git(&world.clone, &["push", "-q", "origin", &format!("{BRANCH}:{lookalike}")]);

    // by default the attempt is continued, whoever asks
    kill_worker(&world);
    let continued = dispatch(&world, &[]).ok();
    assert_eq!((continued["claim"].clone(), continued["tier"].clone()), (json!("held"), json!(1)));
    assert_eq!(continued["worktree"], json!(old));
    assert!(continued.get("discarded").is_none());
    assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{BRANCH}")]), pushed);

    let started = dispatch(&world, &["--start", "fresh"]).ok();
    assert_eq!((started["claim"].clone(), started["tier"].clone()), (json!("held"), json!(3)));
    assert_eq!(started["prompt"], "fresh");
    let discarded = &started["discarded"];
    assert_eq!(discarded["closed_prs"], json!([70]));
    assert_eq!(discarded["deleted_branches"], json!([BRANCH]));
    assert_eq!(discarded["removed_worktree"], json!(old));
    assert!(!old.exists());
    let heads = world.remote(&["for-each-ref", "--format=%(refname)", "refs/heads"]);
    assert!(!heads.lines().any(|head| head == format!("refs/heads/{BRANCH}")), "{heads}");
    assert!(heads.lines().any(|head| head == lookalike), "{heads}");
    let gh = world.gh();
    let state = |number: u64| {
        let prs = gh["prs"].as_array().unwrap();
        prs.iter().find(|pr| pr["number"] == number).unwrap()["state"].clone()
    };
    assert_eq!((state(70), state(71)), (json!("closed"), json!("open")));

    let new = PathBuf::from(started["worktree"].as_str().unwrap());
    assert_eq!(git(&new, &["rev-parse", "HEAD"]), world.remote(&["rev-parse", "refs/heads/main"]));
    assert!(!continues(&brief(&new)));
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn recovery_of_an_issue_never_issued_says_there_is_no_such_issue() {
    let world = World::new();
    world.with_gh(|gh| gh["missing"] = json!({"999999999": 0}));
    let error = world.afk(&["recovery", "--issue", "999999999"]).error();
    assert_eq!(error, "no such issue: #999999999 (HTTP 404)");
}

#[test]
fn recovery_of_a_transferred_issue_names_where_it_went() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.with_gh(|gh| gh["transferred"] = json!({"7": 0}));
    let error = world.afk(&["recovery", "--issue", "7"]).error();
    assert_eq!(error, "issue #7 was transferred to acme/elsewhere#1007");
}

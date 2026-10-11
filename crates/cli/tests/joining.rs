mod common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args =
        ["tick", "--instance", "alpha", "--host", "h1", "--now", &now, "--worker-command", "agent"];
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

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == 7).expect("a worktree of issue 7");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// A world whose worker on issue 7, dispatched at 1000, pushed its work and
/// opened PR #70 — and stopped there, short of the landing train.
fn joining() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done", "heartbeat done"]);
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    world
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

fn joins_only(prompt: &str) -> bool {
    prompt.contains("PR #70 is open for it") && !prompt.contains("**Implement**")
}

/// The handles of the terminals open in `dir`.
fn open_terminals(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).map(|t| t["handle"].as_str().unwrap().to_string()).collect()
}

/// The records of a restart PR #70 carries.
fn restarts(world: &World) -> Vec<String> {
    world.comments(70).into_iter().filter(|body| body.contains("convoy:join")).collect()
}

fn pr_is_open(world: &World) -> bool {
    world.gh()["prs"][0]["state"] == "open"
}

fn restart(world: &World, now: &str) -> common::Answer {
    let args = ["restart", "--issue", "7", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&[&args[..], &["--worker-command", "agent"]].concat())
}

#[test]
fn a_silent_joining_worker_is_nudged_restarted_once_nudged_and_then_escalated() {
    let world = joining();
    let dir = worktree(&world);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("scratch.txt"), "uncommitted").unwrap();
    let first = open_terminals(&world, &dir);
    world.watch_pushes(None);
    world.orca_calls();

    // nudge → grace
    assert_eq!(did(&tick(&world, 1090)), ["watch 7 nudged", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 1180))[0], "watch 7 nothing");
    assert_eq!(restarts(&world), [] as [String; 0]);
    assert!(!joins_only(&brief(&dir)));

    // → restart: recorded on the PR, a new worker in the same worktree, briefed only to join
    assert_eq!(did(&tick(&world, 1390)), ["watch 7 restarted", "heartbeat done"]);
    assert_eq!(restarts(&world).len(), 1);
    assert!(restarts(&world)[0].starts_with("<!--convoy:join restarted=1390-->\n"));
    let second = open_terminals(&world, &dir);
    assert_eq!(second.len(), 1, "the idle session was closed");
    assert_ne!(second, first);
    assert!(joins_only(&brief(&dir)), "{}", brief(&dir));
    assert!(brief(&dir).contains("afkconvoy land --issue 7"));

    // → grace → nudge → grace
    assert_eq!(did(&tick(&world, 1480))[0], "watch 7 nothing");
    assert_eq!(did(&tick(&world, 1690))[0], "watch 7 nudged");
    assert_eq!(did(&tick(&world, 1780))[0], "watch 7 nothing");
    assert_eq!(world.claims().len(), 1);

    // → escalate, with everything kept
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!(["", "Proceed? [y/n]", ""]);
        }
    });
    assert_eq!(did(&tick(&world, 1990)), ["watch 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    let said = world.comments(7);
    let escalation = said.iter().find(|body| body.contains("convoy:escalation")).unwrap();
    assert!(escalation.contains("opened PR #70 and stopped before it was on the landing train"));
    assert!(escalation.contains("a nudge, one restart, and a second nudge"), "{escalation}");
    assert!(escalation.contains("Proceed? [y/n]"), "{escalation}");
    assert_eq!(restarts(&world).len(), 1, "one restart per PR");

    // the PR, the branch and the worktree were untouched at every rung
    assert!(pr_is_open(&world));
    assert_eq!(world.state(7), "open");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/tester/issue-7-issue-7"]), head);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(dir.join("scratch.txt")).unwrap(), "uncommitted");
    let calls: Vec<String> = world.orca_calls().iter().map(|call| call[..2].join(" ")).collect();
    assert!(!calls.iter().any(|call| call == "worktree rm" || call == "worktree create"));
    let pushes = world.pushes();
    assert!(pushes.iter().all(|push| push.starts_with("refs/convoy/")), "{pushes:?}");
}

#[test]
fn the_loop_says_which_worker_it_restarted_and_which_claim_it_escalated() {
    let world = joining();
    tick(&world, 1090);
    let run = |now: &str| {
        let args = ["run", "--host", "h1", "--worker-command", "agent", "--second-ms", "0"];
        let more = ["--instance", "alpha", "--ticks", "1", "--now", now];
        world.afk_lines(&[&args[..], &more[..]].concat()).0
    };
    assert_eq!(run("1390"), ["tick 1: restarted #7 · holding 1/2 · frontier 0 · next in 90s"]);
    tick(&world, 1690);
    assert_eq!(run("1990"), ["tick 1: escalated #7 · holding 0/2 · frontier 0 · next in 90s"]);
}

#[test]
fn a_second_restart_is_refused_by_hand_and_by_any_host_that_reads_the_pr() {
    let world = joining();
    // not before the nudge, nor within its grace
    assert!(restart(&world, "1090").error().contains("it was not nudged yet"));
    tick(&world, 1090);
    assert!(restart(&world, "1180").error().contains("sign of life within the grace period"));
    assert_eq!(restarts(&world), [] as [String; 0]);

    let restarted = restart(&world, "1390").ok();
    assert_eq!((&restarted["action"], &restarted["pr"]), (&json!("restarted"), &json!(70)));
    assert_eq!(restarted["prompt"], "join");
    assert_eq!(restarts(&world).len(), 1);

    // the new worker goes silent too: nudged, and past that there is no second restart
    assert_eq!(did(&tick(&world, 1690))[0], "watch 7 nudged");
    let refused = restart(&world, "1990").error();
    assert!(refused.contains("already had its one restart"), "{refused}");
    assert_eq!(restarts(&world).len(), 1);
    assert_eq!(open_terminals(&world, &worktree(&world)).len(), 1);

    // a restart another host recorded counts here just the same
    let elsewhere = joining();
    elsewhere.comment(70, "<!--convoy:join restarted=1100-->\nrestarted on another host");
    assert_eq!(did(&tick(&elsewhere, 1400))[0], "watch 7 nudged");
    assert_eq!(did(&tick(&elsewhere, 1700))[0], "watch 7 done", "escalated, not restarted");
    assert_eq!(restarts(&elsewhere).len(), 1);
    assert!(pr_is_open(&elsewhere));
}

#[test]
fn a_worker_whose_terminal_is_gone_is_continued_onto_the_join_and_keeps_its_restart() {
    let world = joining();
    let dir = worktree(&world);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
    world.orca_calls();

    assert_eq!(did(&tick(&world, 1090)), ["watch 7 continued", "heartbeat done"]);
    assert_eq!(restarts(&world), [] as [String; 0], "a continuation is not the restart");
    assert!(joins_only(&brief(&dir)));
    assert_eq!(open_terminals(&world, &dir).len(), 1);
    let calls: Vec<String> = world.orca_calls().iter().map(|call| call[..2].join(" ")).collect();
    assert!(!calls.contains(&"worktree create".to_string()), "{calls:?}");
    assert_eq!(world.claims().len(), 1);

    // the join still has its whole ladder: nudge, then the one restart
    assert_eq!(did(&tick(&world, 1180))[0], "watch 7 nudged");
    assert_eq!(did(&tick(&world, 1480))[0], "watch 7 restarted");
    assert_eq!(restarts(&world).len(), 1);
}

#[test]
fn with_no_worktree_on_this_host_the_join_is_continued_from_the_pushed_branch() {
    let world = joining();
    let head = world.remote(&["rev-parse", "refs/heads/tester/issue-7-issue-7"]);
    world.with_orca(|orca| {
        orca["worktrees"] = json!([]);
        orca["terminals"] = json!([]);
    });
    assert_eq!(did(&tick(&world, 1090))[0], "watch 7 continued");
    let dir = worktree(&world);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert!(joins_only(&brief(&dir)));
    assert_eq!(restarts(&world), [] as [String; 0]);
    assert!(pr_is_open(&world));
}

#[test]
fn a_worker_at_work_on_its_join_is_left_with_no_github_read() {
    let world = joining();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_000_000});
            terminal["lastOutputAt"] = json!(1_170_000);
        }
    });
    // the tick that first sees the PR brings the status board up to date
    assert_eq!(did(&tick(&world, 1090)), ["watch 7 nothing", "heartbeat done"]);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1180)), ["watch 7 nothing", "heartbeat done"]);
    let calls: Vec<String> = world.gh_calls().iter().map(|call| call.join(" ")).collect();
    assert!(!calls.iter().any(|call| call.contains("/comments")), "{calls:?}");
    assert_eq!(calls.iter().filter(|call| call.starts_with("pr list")).count(), 1, "{calls:?}");
}

#[test]
fn a_worker_stopped_on_a_join_conflict_is_on_the_same_rungs_with_its_merge_left_as_it_is() {
    let world = joining();
    let dir = worktree(&world);
    commit(&dir, "README.md", "seven's\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.land_elsewhere("README.md");
    git(&dir, &["fetch", "-q", "origin"]);
    let merge = world.command("git", &dir).args(["merge", "-q", "origin/main"]).output().unwrap();
    assert!(!merge.status.success(), "the join conflicts");
    let unmerged = || git(&dir, &["diff", "--name-only", "--diff-filter=U"]);
    assert_eq!(unmerged(), "README.md");

    assert_eq!(did(&tick(&world, 1090))[0], "watch 7 nudged");
    assert_eq!(did(&tick(&world, 1390))[0], "watch 7 restarted");
    assert_eq!(unmerged(), "README.md", "the merge in progress is the new worker's to finish");
    assert!(brief(&dir).contains("A merge left in progress is a"));
    assert_eq!(did(&tick(&world, 1690))[0], "watch 7 nudged");
    assert_eq!(did(&tick(&world, 1990))[0], "watch 7 done");
    assert_eq!(unmerged(), "README.md");
    assert!(pr_is_open(&world));
}

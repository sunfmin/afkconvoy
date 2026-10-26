use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

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

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == 7).expect("a worktree of issue 7");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// A world whose worker on issue 7, dispatched at 700 — a grace period and
/// more before anything asks after it — pushed its work and opened PR #70,
/// and stopped there, short of the landing train.
fn joining() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    assert_eq!(did(&tick(&world, 700)), ["dispatch 7 done", "heartbeat done"]);
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

/// A stopped issue worker with a PR and a recorded green gate, ready to join.
fn gated() -> World {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    let gate = world.afk_in(&dir, &["gate", "--train", "--now", "1040"]).ok();
    assert_eq!((&gate["status"], &gate["recorded"]), (&json!("green"), &json!(true)));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
            terminal["lastOutputAt"] = json!(1_050_000);
        }
    });
    world
}

#[test]
fn a_joined_worker_leaves_health_while_the_train_waits_to_land() {
    let world = gated();
    let dir = worktree(&world);
    let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1050"]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");
    let train = world.root.join("wt/convoy-train");
    for now in [1300, 1600, 1900, 2200, 4900, 8500] {
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                if terminal["worktreePath"] == train.to_str().unwrap() {
                    terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
                    terminal["lastOutputAt"] = json!(now * 1000);
                }
            }
        });
        let ticked = tick(&world, now);
        assert_eq!(ticked["errors"], json!([]), "{ticked}");
        let doctor = world.afk(&["doctor", "--now", &now.to_string()]).json;
        let health = &doctor["host"]["host_kit"]["worker_health"];
        assert!(
            health["workers"].as_array().unwrap().iter().all(|w| w["path"] != json!(dir)),
            "{health}"
        );
        assert_eq!(doctor["claims"][0]["status"], "joined", "{doctor}");
        assert_eq!(world.claims(), [(7, "alpha".into())]);
        assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
        assert!(pr_is_open(&world));
        assert_eq!(world.remote(&["rev-parse", "refs/convoy/train/line/main"]), joined["commit"]);
    }
    assert!(!world.root.join("diagnosis-asked").exists());
    let landed = world.afk_in(&train, &["land", "--train", "--now", "8600"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    tick(&world, 8700);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
}

#[test]
fn joining_retires_existing_health_even_when_its_recovery_window_has_expired() {
    for (attempts, next_tick, broken_gitdir) in
        [(0, 1400, false), (3, 2200, false), (3, 5000, true)]
    {
        let world = gated();
        let dir = worktree(&world);
        tick(&world, 1300);
        let path = world.cache().join("afkconvoy/worker-health.json");
        let mut book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(book["workers"][0]["path"], json!(dir), "{book}");
        book["workers"][0]["attempts"] = json!(attempts);
        book["observations_failed"] = json!([[dir, "old diagnosis failure", 1600]]);
        std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
        let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1350"]).ok();
        assert_eq!(joined["outcome"], "joined", "{joined}");
        if broken_gitdir {
            // A finished worker's local checkout need not remain usable.
            std::fs::write(dir.join(".git"), "gitdir: /missing/finished-worker\n").unwrap();
        }
        let ticked = tick(&world, next_tick);
        assert_eq!(ticked["errors"], json!([]), "{ticked}");
        let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(book["workers"], json!([]), "{book}");
        assert_eq!(book["observations_failed"], json!([]), "{book}");
        assert!(!world.root.join("diagnosis-asked").exists());
        assert_eq!(world.claims(), [(7, "alpha".into())]);
        assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
        assert!(pr_is_open(&world));
    }
}

#[test]
fn a_landing_pushed_before_closing_the_issue_also_retires_expired_health() {
    let world = gated();
    let dir = worktree(&world);
    tick(&world, 1300);
    let path = world.cache().join("afkconvoy/worker-health.json");
    let mut book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    book["workers"][0]["attempts"] = json!(3);
    std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
    let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1350"]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");
    // The train's push succeeds, but it has not closed the PR or issue yet.
    let line = world.remote(&["rev-parse", "refs/convoy/train/line/main"]);
    git(&world.clone, &["fetch", "-q", "origin", &line]);
    git(&world.clone, &["push", "-q", "origin", &format!("{line}:refs/heads/main")]);
    assert_eq!(world.state(7), "open");
    assert!(pr_is_open(&world));

    let ticked = tick(&world, 5000);
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(did(&ticked).contains(&"settle 7 done".to_string()), "{ticked}");
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
    let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(book["workers"], json!([]), "{book}");
    assert!(!world.root.join("diagnosis-asked").exists());
}

#[test]
fn a_stopped_screen_and_a_complete_diagnosis_do_not_exempt_unfinished_work() {
    for outcome in ["open", "advanced", "none"] {
        let world = gated();
        let dir = worktree(&world);
        if outcome == "advanced" {
            let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1050"]).ok();
            assert_eq!(joined["outcome"], "joined", "{joined}");
            commit(&dir, "more.txt", "more work\n");
            git(&dir, &["push", "-q", "origin", "HEAD"]);
        } else if outcome == "none" {
            world.with_gh(|gh| gh["prs"] = json!([]));
        }
        world.hold_every_harness();
        std::fs::write(
            world.root.join("diagnosis-answer"),
            json!({"status": "observed", "class": "complete", "scope": "worker",
                "evidence": [], "action": "complete", "missing": [], "retry_after": null})
            .to_string(),
        )
        .unwrap();
        for now in [1300, 1600, 1900, 2200, 2500] {
            tick(&world, now);
        }
        let path = world.cache().join("afkconvoy/worker-health.json");
        let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let worker =
            book["workers"].as_array().unwrap().iter().find(|w| w["path"] == json!(dir)).unwrap();
        assert_eq!(worker["diagnosis"]["class"], "complete", "{book}");
        assert_eq!(worker["attempts"], 3, "{book}");
        assert_eq!(worker["action"], "waiting for new evidence", "{book}");
        assert_eq!(world.claims(), [(7, "alpha".into())]);
    }
}

fn restart(world: &World, now: &str) -> common::Answer {
    let args = ["restart", "--issue", "7", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&[&args[..], &[]].concat())
}

fn manual_nudge(world: &World, now: &str) {
    world.afk(&["nudge", "7", "--instance", "alpha", "--host", "h1", "--now", now]).ok();
}

#[test]
fn a_silent_joining_worker_waits_for_diagnosis_without_consuming_its_restart() {
    let world = joining();
    let dir = worktree(&world);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("scratch.txt"), "uncommitted").unwrap();
    let first = open_terminals(&world, &dir);
    for now in [1090, 1390, 1690] {
        assert_eq!(did(&tick(&world, now))[0], "watch 7 nothing");
    }
    assert_eq!(open_terminals(&world, &dir), first);
    assert_eq!(restarts(&world), [] as [String; 0]);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(dir.join("scratch.txt")).unwrap(), "uncommitted");
    assert_eq!(world.claims().len(), 1);
    assert!(pr_is_open(&world));
}

#[test]
fn the_loop_keeps_the_joining_claim_while_diagnosis_is_unavailable() {
    let world = joining();
    world.say_concurrency(&world.clone, 2);
    tick(&world, 1090);
    let args = [
        "run",
        "--foreground",
        "--host",
        "h1",
        "--second-ms",
        "0",
        "--instance",
        "alpha",
        "--ticks",
        "1",
        "--now",
        "1390",
    ];
    let lines = world.afk_lines(&args).0;
    assert!(lines[0].contains("nothing to do · holding 1/2"), "{lines:?}");
    assert_eq!(world.claims().len(), 1);
    assert!(restarts(&world).is_empty());
}

#[test]
fn a_second_restart_is_refused_by_hand_and_by_any_host_that_reads_the_pr() {
    let world = joining();
    // not before the nudge, nor within its grace
    assert!(restart(&world, "1090").error().contains("it was not nudged yet"));
    manual_nudge(&world, "1090");
    assert!(restart(&world, "1180").error().contains("sign of life within the grace period"));
    assert_eq!(restarts(&world), [] as [String; 0]);

    let restarted = restart(&world, "1390").ok();
    assert_eq!((&restarted["action"], &restarted["pr"]), (&json!("restarted"), &json!(70)));
    assert_eq!(restarted["prompt"], "join");
    assert_eq!(restarts(&world).len(), 1);

    // the new worker goes silent too: nudged, and past that there is no second restart
    manual_nudge(&world, "1690");
    let refused = restart(&world, "1990").error();
    assert!(refused.contains("already had its one restart"), "{refused}");
    assert_eq!(restarts(&world).len(), 1);
    assert_eq!(open_terminals(&world, &worktree(&world)).len(), 1);

    // a restart another host recorded counts here just the same
    let elsewhere = joining();
    elsewhere.comment(70, "<!--convoy:join restarted=1100-->\nrestarted on another host");
    manual_nudge(&elsewhere, "1400");
    assert!(restart(&elsewhere, "1700").error().contains("already had its one restart"));
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
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.contains(&"worktree create".to_string()), "{calls:?}");
    assert_eq!(world.claims().len(), 1);

    // the join still has its whole ladder: a grace period from its start,
    // the nudge, then the one restart
    assert_eq!(did(&tick(&world, 1180))[0], "watch 7 nothing");
    manual_nudge(&world, "1390");
    restart(&world, "1690").ok();
    assert_eq!(restarts(&world).len(), 1);
}

#[test]
fn a_worker_is_continued_onto_its_own_pr_not_a_fork_s_numbered_higher() {
    let world = joining();
    world.fork_pr(99, 7, "patch-1");
    let dir = worktree(&world);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });

    assert_eq!(did(&tick(&world, 1090)), ["watch 7 continued", "heartbeat done"]);
    let brief = brief(&dir);
    assert!(joins_only(&brief) && !brief.contains("#99"), "{brief}");
    assert_eq!(world.comments(99), [] as [String; 0]);
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

    manual_nudge(&world, "1090");
    restart(&world, "1390").ok();
    assert_eq!(unmerged(), "README.md", "the merge in progress is the new worker's to finish");
    assert!(brief(&dir).contains("A merge left in progress is a"));
    assert_eq!(did(&tick(&world, 1690))[0], "watch 7 nothing");
    assert_eq!(did(&tick(&world, 1990))[0], "watch 7 nothing");
    assert_eq!(unmerged(), "README.md");
    assert!(pr_is_open(&world));
}

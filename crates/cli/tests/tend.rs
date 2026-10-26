use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

const LINE: &str = "refs/convoy/train/line/main";

fn tick(world: &World, now: &str) -> Value {
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        // a worker asked after, and left or nudged, is not what these tests are about
        .filter(|step| step["do"] != "board")
        .filter(|step| {
            !((step["do"] == "judge" || step["do"] == "watch") && step["outcome"] != "done")
        })
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world whose fleet holds two claims at once, with workers dispatched on
/// `issues` at 1000.
fn fleet(issues: &[u64]) -> World {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
    world.set_gate("true");
    for &n in issues {
        world.issue(n, &["ready-for-agent"]);
    }
    assert_eq!(tick(&world, "1000")["errors"], json!([]));
    world
}

fn worktree_of(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == issue).expect("a worktree of the issue");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The worker on `issue` finishes: commits, opens PR #`issue`0 and joins the
/// train → the join commit.
fn finish(world: &World, issue: u64) -> String {
    let dir = worktree_of(world, issue);
    commit(&dir, &format!("f{issue}.txt"), "x\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(issue * 10, issue, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    let answer = world.afk_in(&dir, &["land", "--issue", &issue.to_string(), "--now", "1050"]).ok();
    assert_eq!(answer["outcome"], "joined", "{answer}");
    answer["commit"].as_str().unwrap().to_string()
}

fn train_worktree(world: &World) -> PathBuf {
    world.root.join("wt/convoy-train")
}

#[test]
fn a_failed_train_agent_is_diagnosed_on_the_first_tick_and_keeps_its_train() {
    let world = fleet(&[7]);
    let joined = finish(&world, 7);
    tick(&world, "1090");
    world.hold_every_harness();
    let train = train_worktree(&world);
    let mut agent: Value =
        serde_json::from_str(include_str!("fixtures/orca-failed-agent.json")).unwrap();
    agent["stateStartedAt"] = json!(1099000);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == json!(train) {
                terminal["agent"] = agent.clone();
                terminal["screen"] = json!(["The train session failed"]);
            }
        }
    });
    std::fs::write(
        world.root.join("diagnosis-answer"),
        json!({
            "status":"unknown", "class":"unknown", "scope":"worker", "action":"unknown",
            "evidence":[], "missing":["fault cause"], "retry_after":null
        })
        .to_string(),
    )
    .unwrap();
    tick(&world, "1100");
    assert!(world.root.join("diagnosis-asked").exists());
    assert_eq!(world.remote(&["rev-parse", LINE]), joined);
    // There is no live host runner in this world; doctor still reports the
    // train's evidence alongside that independent critical finding.
    let health = world.afk(&["doctor", "--now", "1101", "--host", "h1"]).json;
    let workers = health["host"]["host_kit"]["worker_health"]["workers"].as_array().unwrap();
    let worker = workers.iter().find(|w| w["path"] == json!(train)).unwrap();
    assert_eq!(worker["attempts"], 1);
    assert_eq!(worker["orca"]["outcome"], "failure");
    assert_eq!(worker["harness"], "codex");
    assert_eq!(worker["recorded_harness"], "claude-code");

    // programStatus gets fresh observation times without a new failed turn.
    // It must not consume another attempt before the existing next check.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == json!(train) {
                terminal["agent"]["programStatus"]["observedAt"] = json!(1102000);
            }
        }
    });
    tick(&world, "1102");
    let health = world.afk(&["plan", "--now", "1103"]).ok();
    let workers = health["host_kit"]["worker_health"]["workers"].as_array().unwrap();
    let worker = workers.iter().find(|w| w["path"] == json!(train)).unwrap();
    assert_eq!(worker["attempts"], 1);
    assert_eq!(worker["orca"]["programStatus"]["observedAt"], 1102000);

    let handle = terminals(&world, &train)[0]["handle"].clone();
    let head = commit(&train, "repair.txt", "keep the train's repair\n");
    std::fs::write(train.join("unfinished.txt"), "keep this too\n").unwrap();
    tick(&world, "4699");
    assert_eq!(terminals(&world, &train)[0]["handle"], handle, "the health hold has a bound");
    let ticked = tick(&world, "4700");
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1);
    assert_ne!(open[0]["handle"], handle, "health no longer holds this train");
    assert_eq!(open[0]["command"], common::trusted("agent"));
    assert_eq!(git(&train, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(train.join("unfinished.txt")).unwrap(), "keep this too\n");
    assert_eq!(world.remote(&["rev-parse", LINE]), joined);
    let logs: String = std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert!(logs.contains("worker health held the train worker for 3600 seconds"), "{logs}");
}

#[test]
fn a_new_join_replaces_a_failed_paused_faulted_or_outdated_train_worker() {
    for cause in ["failed", "paused", "faulted", "switched", "switched while busy"] {
        let world = fleet(&[7, 8]);
        finish(&world, 7);
        tick(&world, "1090");
        world.hold_every_harness();
        let train = train_worktree(&world);
        let old = terminals(&world, &train)[0]["handle"].clone();
        let notes = PathBuf::from(git(&train, &["rev-parse", "--absolute-git-dir"]));
        let head = commit(&train, "repair.txt", "keep the train's repair\n");
        std::fs::write(train.join("unfinished.txt"), "unfinished\n").unwrap();
        let expected = match cause {
            "failed" => {
                let mut failed: Value =
                    serde_json::from_str(include_str!("fixtures/orca-failed-agent.json")).unwrap();
                failed["agentType"] = json!("claude-code");
                world.with_orca(|orca| {
                    for terminal in orca["terminals"].as_array_mut().unwrap() {
                        if terminal["handle"] == old {
                            terminal["agent"] = failed.clone();
                            terminal["menu"] = json!(true);
                        }
                    }
                });
                "claude-code"
            }
            "paused" => {
                world.pause("claude-code", 1100, "operator paused this harness");
                "pi"
            }
            "faulted" => {
                let key = std::fs::read_to_string(notes.join("convoy-worker-command-key")).unwrap();
                let path = world.cache().join("afkconvoy/worker-health.json");
                let mut book: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                book["faults"] = json!([{"harness":"claude-code", "command_key":key,
                    "model":null, "scope":"account", "since":1100, "next_check":10000,
                    "evidence":["account disabled"]}]);
                std::fs::write(path, serde_json::to_vec(&book).unwrap()).unwrap();
                "pi"
            }
            _ => {
                world.afk(&["switch", "--harness", "kimi"]).ok();
                if cause == "switched while busy" {
                    world.with_orca(|orca| {
                        for terminal in orca["terminals"].as_array_mut().unwrap() {
                            if terminal["handle"] == old {
                                terminal["agent"] = json!({"state":"working", "agentType":"claude-code", "stateStartedAt":1100000});
                            }
                        }
                    });
                }
                "kimi"
            }
        };
        let tip = finish(&world, 8);
        world.orca_calls();
        let ticked = tick(&world, "1180");
        assert_eq!(ticked["errors"], json!([]), "{cause}: {ticked}");
        let open = terminals(&world, &train);
        assert_eq!(open.len(), 1, "{cause}");
        assert_ne!(open[0]["handle"], old, "{cause}: the old worker must be ended");
        assert_eq!(
            std::fs::read_to_string(notes.join("convoy-harness")).unwrap(),
            expected,
            "{cause}"
        );
        let sent = open[0]["sent"].as_array().unwrap();
        assert_eq!(sent.len(), 1, "{cause}");
        assert!(
            !sent[0]["text"].as_str().unwrap().starts_with("PRs have joined"),
            "a replacement gets its brief"
        );
        let old_sends = world.orca_calls().iter().any(|call| {
            common::asked(call) == "terminal send"
                && call.iter().any(|arg| arg == old.as_str().unwrap())
        });
        assert!(!old_sends, "{cause}: never type into the old session");
        assert_eq!(git(&train, &["rev-parse", "HEAD"]), head, "{cause}");
        assert_eq!(std::fs::read_to_string(train.join("unfinished.txt")).unwrap(), "unfinished\n");
        assert_eq!(world.remote(&["rev-parse", LINE]), tip, "{cause}");
        assert!(
            std::fs::read_to_string(notes.join("convoy-train-told")).unwrap().starts_with(&tip)
        );
    }
}

#[test]
fn switch_all_moves_a_busy_train_worker_and_lists_it_without_a_claim() {
    let world = fleet(&[7]);
    let tip = finish(&world, 7);
    tick(&world, "1090");
    world.hold_every_harness();
    // Train workers hold no claim: switch must find this one independently.
    world.afk(&["release", "--issue", "7", "--instance", "alpha", "--now", "1100"]).ok();
    let train = train_worktree(&world);
    let old = terminals(&world, &train)[0]["handle"].clone();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == old {
                terminal["agent"] =
                    json!({"state":"working", "agentType":"claude-code", "stateStartedAt":1100000});
            }
        }
    });
    let answer =
        world.afk(&["switch", "--harness", "kimi", "--all", "--now", "1180", "--host", "h1"]).ok();
    assert_eq!(answer["failed"], json!([]), "{answer}");
    let moved = answer["switched"].as_array().unwrap();
    assert!(
        moved.iter().any(|worker| worker["worktree"] == json!(train)
            && worker["from"] == "claude-code"
            && worker["to"] == "kimi"),
        "{answer}"
    );
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1);
    assert_ne!(open[0]["handle"], old);
    assert!(open[0]["command"].as_str().unwrap().contains("kimi"));
    assert_eq!(world.remote(&["rev-parse", LINE]), tip);
}

#[test]
fn a_train_pointer_requires_input_readiness_and_a_started_turn_before_recording_told() {
    for menu in [true, false] {
        let world = fleet(&[7, 8]);
        let first = finish(&world, 7);
        tick(&world, "1090");
        let train = train_worktree(&world);
        let notes = PathBuf::from(git(&train, &["rev-parse", "--absolute-git-dir"]));
        finish(&world, 8);
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                if terminal["worktreePath"] == train.to_str().unwrap() {
                    terminal["agent"] = json!({"state":"idle", "stateStartedAt":1_100_000});
                    terminal["menu"] = json!(menu);
                    terminal["screen"] = json!(
                        include_str!("screens/codex-model-menu.txt").lines().collect::<Vec<_>>()
                    );
                    terminal["prompt_stages"] = json!(["input_accepted"]);
                }
            }
        });
        world.orca_calls();
        let ticked = tick(&world, "1180");
        assert!(
            ticked["errors"].as_array().unwrap().iter().any(|e| e["do"] == "tend_train"),
            "{ticked}"
        );
        let told = std::fs::read_to_string(notes.join("convoy-train-told")).unwrap();
        assert!(told.starts_with(&first), "an unproven pointer changed told: {told}");
        let sends =
            world.orca_calls().iter().filter(|call| common::asked(call) == "terminal send").count();
        assert_eq!(sends, usize::from(!menu), "no pointer is typed into a menu");
        assert!(
            notes.join("convoy-worker-error.json").exists(),
            "worker health must see failed delivery on its next tick"
        );
        world.orca_calls();
        tick(&world, "1270");
        assert!(
            !world.orca_calls().iter().any(|call| common::asked(call) == "terminal send"),
            "the next tick leaves the stopped worker to health instead of resending"
        );
    }
}

#[test]
fn a_codex_train_session_has_model_and_effort_evidence_in_plan_and_doctor() {
    let world = fleet(&[7]);
    finish(&world, 7);
    world.hold_every_harness();
    world.kit_models("codex", &["fleet-model"]);
    world.afk(&["switch", "--harness", "codex"]).ok();
    common::write(&world.home(), ".codex/config.toml", "model_reasoning_effort = 'high'\n");
    let ticked = tick(&world, "1090");
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    let train = train_worktree(&world);
    let terminals = terminals(&world, &train);
    assert_eq!(terminals.len(), 1);
    let command = terminals[0]["command"].as_str().unwrap();
    assert!(command.contains("--model 'fleet-model'"), "{command}");
    assert!(command.contains("model_reasoning_effort="), "{command}");
    for args in [
        vec!["plan", "--instance", "alpha", "--now", "1100"],
        vec!["doctor", "--host", "h1", "--now", "1100"],
    ] {
        let result = world.afk(&args);
        let rows = result.json["codex_workers"].as_array().unwrap();
        assert!(
            rows.iter().any(|row| row["worktree"] == json!(train)
                && row["model"] == "fleet-model"
                && row["effort"] == "high"),
            "{rows:?}"
        );
    }
}

/// The terminals open in a worktree.
fn terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let rows = orca["terminals"].as_array().unwrap();
    let here = |t: &&Value| t["open"] == true && t["worktreePath"] == dir.to_str().unwrap();
    rows.iter().filter(here).cloned().collect()
}

fn status(world: &World, issue: u64) -> String {
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1100"]).ok();
    let mine = plan["mine"].as_array().unwrap();
    let row = mine.iter().find(|row| row["number"] == issue).expect("a claim of mine");
    row["status"].as_str().unwrap().to_string()
}

fn state(world: &World, issue: u64) -> String {
    let gh = world.gh();
    let rows = gh["issues"].as_array().unwrap();
    rows.iter().find(|row| row["number"] == issue).unwrap()["state"].as_str().unwrap().to_string()
}

#[test]
fn a_joined_pr_gets_a_train_worker_that_holds_no_claim_and_no_slot() {
    let world = fleet(&[7, 8, 9]);
    let tip = finish(&world, 7);
    world.orca_calls();

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["tend_train done", "heartbeat done"]);
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(ticked["held"], 2, "the train worker takes no slot");
    assert_eq!(world.claims().len(), 2, "…and holds no claim");

    let train = train_worktree(&world);
    let orca = world.orca();
    let row = orca["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == train.to_str().unwrap());
    assert_eq!(row.expect("the train's worktree")["linkedIssue"], Value::Null);
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1, "the worker's is the only terminal there");
    assert_eq!(open[0]["command"], common::trusted("agent"));
    let brief = PathBuf::from(git(&train, &["rev-parse", "--absolute-git-dir"]))
        .join("convoy-worker-prompt.md");
    let sent = open[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0]["text"].as_str().unwrap().contains(brief.to_str().unwrap()));
    assert_eq!(sent[0]["enter"], true);
    let prompt = std::fs::read_to_string(&brief).unwrap();
    assert!(prompt.contains(&format!("{} land --train\n", env!("CARGO_BIN_EXE_afkconvoy"))));
    assert!(prompt.contains("train worker"));
    // cut at the base branch's tip, on a branch of its own
    assert_eq!(
        git(&train, &["rev-parse", "HEAD"]),
        world.remote(&["rev-parse", "refs/heads/main"])
    );
    assert_eq!(world.remote(&["rev-parse", LINE]), tip);
}

#[test]
fn a_train_worker_is_left_at_work_told_what_joined_and_started_again_when_gone() {
    let world = fleet(&[7, 8]);
    finish(&world, 7);
    tick(&world, "1090");
    let train = train_worktree(&world);
    let handle = terminals(&world, &train)[0]["handle"].clone();
    world.orca_calls();

    // it stopped on the tip it was told about: its silence is not this pass's
    assert_eq!(did(&tick(&world, "1180"))[0], "tend_train nothing");

    // more joined while it is at work: it is left alone
    let second = finish(&world, 8);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == handle {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_200_000u64});
                terminal["lastOutputAt"] = json!(1_265_000u64);
            }
        }
    });
    assert_eq!(did(&tick(&world, "1270"))[0], "tend_train nothing");

    // it stopped: it is told that the train holds more
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == handle {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_300_000u64});
            }
        }
    });
    world.orca_calls();
    assert_eq!(did(&tick(&world, "1360"))[0], "tend_train done");
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["handle"], handle, "the same worker");
    let sent = open[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent[1]["text"].as_str().unwrap().starts_with("PRs have joined the landing train"));
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.contains(&"terminal create".to_string()), "{calls:?}");
    assert_eq!(world.remote(&["rev-parse", LINE]), second);
    assert_eq!(did(&tick(&world, "1450"))[0], "tend_train nothing", "told once per tip");

    // its terminal is gone: another is started, in the same worktree
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
    assert_eq!(did(&tick(&world, "1540"))[0], "tend_train done");
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1);
    assert_ne!(open[0]["handle"], handle);
    let trains = world.orca()["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["linkedIssue"].is_null())
        .count();
    assert_eq!(trains, 1, "no second worktree is cut beside it");
}

#[test]
fn a_landed_claim_is_settled_and_the_checkout_follows() {
    let world = fleet(&[7]);
    finish(&world, 7);
    tick(&world, "1090");
    let (train, worktree) = (train_worktree(&world), worktree_of(&world, 7));
    // the train worker lands the train
    let landed =
        world.afk_in(&train, &["--now", "1170", "land", "--train", "--merged-timeout", "5"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    let tip = world.remote(&["rev-parse", "refs/heads/main"]);
    assert_ne!(git(&world.clone, &["rev-parse", "main"]), tip);
    world.watch_pushes(None);

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 done"]);
    assert_eq!(ticked["errors"], json!([]));
    assert!(!worktree.exists(), "the issue's worktree is removed");
    assert!(train.exists(), "the train's worktree is never swept");
    assert_eq!(world.claims(), []);
    assert_eq!(world.pushes(), ["refs/convoy/claim/7"]);
    // what landed is on this machine without anyone pulling
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), tip);
    assert_eq!(git(&world.clone, &["rev-parse", "origin/main"]), tip);
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "");
    assert!(world.clone.join("f7.txt").exists());
}

fn worker_branches(world: &World) -> Vec<String> {
    let listed = world.remote(&["for-each-ref", "--format=%(refname:short)", "refs/heads"]);
    listed.lines().filter(|name| *name != "main").map(str::to_string).collect()
}

/// Lands the train with GitHub slow to show PR #70 merged: the landing leaves
/// its branch on the remote.
fn land_with_the_branch_left(world: &World) {
    world.with_gh(|gh| gh["slow_merge"] = json!([70]));
    let train = train_worktree(world);
    let landed =
        world.afk_in(&train, &["--now", "1170", "land", "--train", "--merged-timeout", "0"]).ok();
    assert_eq!(landed["unmerged"], json!([70]), "{landed}");
    assert_eq!(worker_branches(world).len(), 1);
}

#[test]
fn a_branch_the_landing_left_behind_is_deleted_when_its_claim_is_settled() {
    let world = fleet(&[7]);
    finish(&world, 7);
    tick(&world, "1090");
    land_with_the_branch_left(&world);

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 done"]);
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(worker_branches(&world), [] as [&str; 0]);
    assert_eq!(world.claims(), []);
}

#[test]
fn a_branch_pushed_to_since_it_landed_is_kept() {
    let world = fleet(&[7]);
    finish(&world, 7);
    tick(&world, "1090");
    let worktree = worktree_of(&world, 7);
    land_with_the_branch_left(&world);
    commit(&worktree, "more.txt", "more\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);

    let ticked = tick(&world, "1180");
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(worker_branches(&world).len(), 1, "it holds work nothing landed");
}

/// Puts the train on the base branch as a landing cut right after its push
/// would leave it: nothing else done.
fn cut_after_the_push(world: &World) -> String {
    let tip = world.remote(&["rev-parse", LINE]);
    world.remote(&["update-ref", "refs/heads/main", &tip]);
    tip
}

#[test]
fn a_landing_cut_after_its_push_is_finished_from_the_base_branch() {
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    assert_eq!(state(&world, 7), "open");
    assert_eq!(status(&world, 7), "landed", "never treated as PR-less");

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 done"]);
    assert_eq!(state(&world, 7), "closed");
    assert_eq!(world.claims(), []);
    assert!(world.orca()["worktrees"].as_array().unwrap().is_empty());
}

#[test]
fn a_cut_landing_whose_settling_fails_keeps_the_claim_with_the_issue_closed() {
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    world.with_orca(|orca| orca["rm_fails"] = json!(true));

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 failed", "heartbeat done"]);
    assert_eq!(state(&world, 7), "closed", "the issue first");
    assert_eq!(world.claims().len(), 1, "the claim last: held for the next tick");

    world.with_orca(|orca| orca["rm_fails"] = json!(false));
    assert_eq!(did(&tick(&world, "1270")), ["settle 7 done"]);
    assert_eq!(world.claims(), []);
}

#[test]
fn a_landed_pr_github_still_shows_open_is_closed_naming_its_commit() {
    let world = fleet(&[7]);
    let commit = finish(&world, 7);
    world.with_gh(|gh| gh["slow_merge"] = json!([70]));
    cut_after_the_push(&world);
    assert_eq!(status(&world, 7), "landed");

    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(state(&world, 7), "closed");
    let gh = world.gh();
    assert_eq!(gh["prs"][0]["state"], "closed");
    let comments = world.comments(70);
    assert_eq!(comments.len(), 1);
    assert!(comments[0].contains(&commit) && comments[0].contains("`main`"), "{comments:?}");
}

#[test]
fn an_open_pr_pushed_to_since_it_landed_is_not_a_cut_landing() {
    let world = fleet(&[7]);
    finish(&world, 7);
    world.with_gh(|gh| gh["slow_merge"] = json!([70]));
    let dir = worktree_of(&world, 7);
    commit(&dir, "more.txt", "more\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    cut_after_the_push(&world);
    assert_eq!(status(&world, 7), "joining", "its worker joins again with what it pushed");
    assert_eq!(did(&tick(&world, "1180")), ["heartbeat done"]);
    assert_eq!(state(&world, 7), "open");
}

#[test]
fn a_checkout_that_cannot_follow_never_fails_the_release() {
    // a local base branch that diverged
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    let local = commit(&world.clone, "local.txt", "mine\n");
    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), local, "left as it was");
    assert_eq!(world.claims(), []);

    // changes in the way
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    let before = git(&world.clone, &["rev-parse", "main"]);
    std::fs::write(world.clone.join("f7.txt"), "in the way\n").unwrap();
    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), before);
    assert_eq!(std::fs::read_to_string(world.clone.join("f7.txt")).unwrap(), "in the way\n");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_base_branch_that_is_not_checked_out_follows_by_its_ref() {
    let world = fleet(&[7]);
    finish(&world, 7);
    let tip = cut_after_the_push(&world);
    git(&world.clone, &["checkout", "-q", "-b", "side"]);
    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), tip);
    assert_eq!(
        git(&world.clone, &["rev-parse", "HEAD"]),
        git(&world.clone, &["rev-parse", "side"])
    );
    assert!(!world.clone.join("f7.txt").exists(), "the checkout itself is not touched");
}

#[test]
fn a_new_worker_in_a_worktree_was_never_nudged() {
    let world = fleet(&[7]);
    finish(&world, 7);
    tick(&world, "1090");
    let train = train_worktree(&world);
    let nudged =
        PathBuf::from(git(&train, &["rev-parse", "--absolute-git-dir"])).join("convoy-nudged");
    std::fs::write(&nudged, "1100").unwrap();

    // the worker that was nudged is gone, and another is started in its place
    world.with_orca(|orca| orca["terminals"] = json!([]));
    assert_eq!(did(&tick(&world, "1180"))[0], "tend_train done");
    assert_eq!(terminals(&world, &train).len(), 1);
    assert!(!nudged.exists(), "the record of the nudge went with the worker it was sent to");
}

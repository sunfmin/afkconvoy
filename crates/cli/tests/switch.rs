use crate::common;

use std::path::{Path, PathBuf};

use common::{World, codex_line, commit, git};
use serde_json::{Value, json};

#[test]
fn kit_priority_is_host_wide_idempotent_and_ignores_legacy_repository_choices() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_models("codex", &["gpt-x"]);
    let before: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    let other = world.second_repo();
    for (dir, old) in [(&world.clone, "pi"), (&other, "claude-code")] {
        let git_dir = PathBuf::from(git(dir, &["rev-parse", "--git-common-dir"]));
        let git_dir = if git_dir.is_absolute() { git_dir } else { dir.join(git_dir) };
        std::fs::write(git_dir.join("convoy-chosen-harness"), old).unwrap();
    }
    let moved = world.afk_in(&world.root, &["switch", "--harness", "codex"]).ok();
    assert_eq!(moved["order"], json!(["codex", "claude-code", "pi", "dsh"]));
    assert_eq!(moved["switched"], json!([]));
    let after: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    for held in after["harnesses"].as_array().unwrap() {
        assert!(before["harnesses"].as_array().unwrap().contains(held));
    }
    let again = world.afk_in(&other, &["switch", "--harness", "codex"]).ok();
    assert_eq!(again["changed"], false);
    assert_eq!(
        after,
        serde_json::from_slice::<Value>(&std::fs::read(world.kit_path()).unwrap()).unwrap()
    );
    world.issue(7, &["ready-for-agent", "harness:pi", "model:another-harness-model"]);
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["cast"]["harness"], "codex");
    assert_eq!(plan["frontier"][0]["cast"]["model"], "gpt-x");
    assert!(world.afk(&["switch", "--clear"]).error().contains("--clear"));
    let back = world.afk_in(&other, &["switch", "--harness", "claude-code"]).ok();
    assert_eq!(back["order"], json!(["claude-code", "codex", "pi", "dsh"]));
}

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

fn switch(world: &World, now: &str, rest: &[&str]) -> common::Answer {
    let args = ["switch", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&[&args[..], rest, &[]].concat())
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

/// The commands of the terminals open in `dir`.
fn open_commands(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    let open = here.filter(|t| t["open"] == true);
    open.map(|t| t["command"].as_str().unwrap_or("").to_string()).collect()
}

/// A world whose worker on issue 7 pushed its work, opened PR #70, and sits
/// at its prompt, its harness's access turned off.
fn blocked() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    // Every harness is held once the worker started: no caster held 7 back.
    world.hold_every_harness();
    let dir = worktree(&world, 7);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] =
                json!(["", "Your organization has disabled Claude Code access", "> ", ""]);
        }
    });
    world
}

#[test]
fn a_switch_moves_the_worker_onto_the_harness_in_the_same_worktree_and_records_it() {
    let world = blocked();
    let dir = worktree(&world, 7);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    world.orca_calls();

    let answer = switch(&world, "800", &["7", "--harness", "codex"]).ok();
    let moved = &answer["switched"][0];
    assert_eq!(moved["issue"], 7);
    assert_eq!((&moved["from"], &moved["to"]), (&json!("claude-code"), &json!("codex")));
    assert_eq!(moved["worktree"], json!(dir));
    assert_eq!(moved["pr"], 70);
    assert_eq!(answer["order"][0], "codex");

    // the old worker's terminal is closed, a codex worker is in the same worktree
    assert_eq!(open_commands(&world, &dir), [codex_line(&world, &dir)]);
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.iter().any(|call| call == "worktree rm" || call == "worktree create"));
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(world.claims().len(), 1);

    // its brief says what it continues from
    let said = brief(&dir);
    assert!(said.contains("## You continue earlier work"), "{said}");
    assert!(said.contains("A worker on `claude-code` was at this before you"), "{said}");
    assert!(said.contains("1 on the branch ahead of `origin/main`"), "{said}");
    assert!(said.contains("#70 is open"), "{said}");
    assert!(said.contains("Your organization has disabled Claude Code access"), "{said}");

    // the PR records the switch, which is no restart
    let records: Vec<String> =
        world.comments(70).into_iter().filter(|body| body.contains("convoy:join")).collect();
    assert_eq!(records.len(), 1);
    assert!(records[0].starts_with("<!--convoy:join switched=800/claude-code/codex/alpha-->"));
    assert!(records[0].contains("switched from `claude-code` to `codex` by `alpha`"));

    // every worker this host starts for the repo from now on is a codex one
    world.issue(8, &["ready-for-agent", "bug", "harness:codex"]);
    tick(&world, 900);
    assert_eq!(
        open_commands(&world, &worktree(&world, 8)),
        [codex_line(&world, &worktree(&world, 8))]
    );

    // `--clear` forgets the choice: the worker it names runs what its casting
    // says again — the host kit's first, for none — recorded as a switch is
    let eight = worktree(&world, 8);
    commit(&eight, "eight.txt", "8\n");
    git(&eight, &["push", "-q", "origin", "HEAD"]);
    world.pr(80, 8, &git(&eight, &["symbolic-ref", "--short", "HEAD"]));
    let answer = switch(&world, "1000", &["8", "--harness", "claude-code"]).ok();
    let back = &answer["switched"][0];
    assert_eq!(answer["order"][0], "claude-code");
    assert_eq!((&back["from"], &back["to"]), (&json!("codex"), &json!("claude-code")));
    assert_eq!(back["pr"], 80);
    assert_eq!(open_commands(&world, &eight), ["agent"]);
    let records: Vec<String> =
        world.comments(80).into_iter().filter(|body| body.contains("convoy:join")).collect();
    assert_eq!(records.len(), 1);
    assert!(records[0].contains("switched from `codex` to `claude-code` by `alpha`"));
    let said = switch(&world, "1000", &["--harness", "config"]).error();
    assert!(said.contains("`--harness config` is gone: switch"), "{said}");
    assert_eq!(
        open_commands(&world, &dir),
        [codex_line(&world, &dir)],
        "a worker it names not is left alone"
    );
}

#[test]
fn a_harness_that_cannot_be_started_is_refused_before_any_worker_is_ended() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.logged_out("codex");
    let said = switch(&world, "800", &["--all", "--harness", "codex"]).error();
    assert!(said.contains("the harness `codex` cannot be started here"), "{said}");
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    assert!(!brief(&dir).contains("You continue earlier work"));
    // nothing was chosen: the next start is on its casting still, which a
    // switch would have overruled
    world.issue(8, &["ready-for-agent", "bug", "harness:claude-code"]);
    tick(&world, 900);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), ["agent"]);
}

#[test]
fn all_moves_every_claim_of_the_instance_and_a_claim_it_does_not_hold_is_refused() {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    world.issue(8, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    world.hold_every_harness();
    let (seven, eight) = (worktree(&world, 7), worktree(&world, 8));
    // one worker is already gone: it is started again all the same
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == eight.to_str().unwrap() {
                terminal["open"] = json!(false);
            }
        }
    });

    let said = switch(&world, "800", &["9", "--harness", "codex"]).ok();
    assert!(!said["failed"].as_array().unwrap().is_empty(), "{said}");
    assert_eq!(open_commands(&world, &seven), ["agent"]);

    let moved = switch(&world, "800", &["--all", "--harness", "codex"]).ok();
    let switched = moved["switched"].as_array().unwrap();
    let issues: Vec<&Value> = switched.iter().map(|one| &one["issue"]).collect();
    assert_eq!(issues, [&json!(7), &json!(8)], "{moved}");
    assert_eq!(moved["failed"], json!([]));
    assert_eq!(open_commands(&world, &seven), [codex_line(&world, &seven)]);
    assert_eq!(open_commands(&world, &eight), [codex_line(&world, &eight)]);
    assert!(brief(&eight).contains("its terminal was already gone"), "{}", brief(&eight));
    assert_eq!(world.claims().len(), 2);
}

#[test]
fn a_failed_close_preserves_work_and_all_continues_with_the_other_workers() {
    let world = blocked();
    world.issue(8, &["ready-for-agent", "bug", "harness:claude-code"]);
    tick(&world, 750);
    let (seven, eight) = (worktree(&world, 7), worktree(&world, 8));
    let uncommitted = seven.join("unfinished.txt");
    std::fs::write(&uncommitted, "work in progress\n").unwrap();
    let head = git(&seven, &["rev-parse", "HEAD"]);
    world.with_orca(|orca| orca["close_fails"] = json!([seven]));
    world.orca_calls();

    let moved = switch(&world, "800", &["--all", "--harness", "codex"]).ok();
    assert_eq!(moved["switched"][0]["issue"], 8, "{moved}");
    assert_eq!(moved["failed"][0]["issue"], 7, "{moved}");
    assert!(moved["failed"][0]["error"].as_str().unwrap().contains("terminal_close_failed"));
    assert_eq!(open_commands(&world, &seven), ["agent"]);
    assert_eq!(open_commands(&world, &eight), [codex_line(&world, &eight)]);
    assert_eq!(std::fs::read_to_string(uncommitted).unwrap(), "work in progress\n");
    assert_eq!(git(&seven, &["rev-parse", "HEAD"]), head);
    assert_eq!(world.claims().len(), 2);
    let git_dir = PathBuf::from(git(&seven, &["rev-parse", "--absolute-git-dir"]));
    assert_eq!(std::fs::read_to_string(git_dir.join("convoy-harness")).unwrap(), "claude-code");
    let calls = world.orca_calls();
    assert!(!calls.iter().any(|call| {
        common::asked(call) == "terminal create"
            && call.iter().any(|arg| arg == &format!("path:{}", seven.display()))
    }));
}

#[test]
fn an_acknowledged_close_that_leaves_the_terminal_is_no_permission_for_a_second_writer() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    let answer = switch(&world, "800", &["7", "--harness", "codex"]).ok();
    let error = answer["failed"][0]["error"].as_str().unwrap();
    assert!(error.contains("termination is unconfirmed"), "{error}");
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    assert_eq!(world.claims().len(), 1);
}

fn diagnosis(world: &World, class: &str, action: &str, scope: &str, evidence: &[&str]) {
    std::fs::write(
        world.root.join("diagnosis-answer"),
        json!({
            "status": "observed", "class": class, "scope": scope, "evidence": evidence,
            "action": action, "missing": [], "retry_after": null
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn screen_faults_are_diagnosed_before_automatic_continuation_and_keep_the_scene() {
    let world = blocked();
    let dir = worktree(&world, 7);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    let before = std::fs::read(world.kit_path()).unwrap();
    diagnosis(
        &world,
        "disabled",
        "continue",
        "account",
        &["Your organization has disabled Claude Code access"],
    );
    tick(&world, 800);
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    assert!(!world.root.join("diagnosis-asked").exists());
    tick(&world, 1100);
    let asked = std::fs::read_to_string(world.root.join("diagnosis-asked")).unwrap();
    assert!(asked.contains("Your organization has disabled Claude Code access"));
    assert!(asked.contains("orca terminal read --screen"));
    assert!(asked.contains("--no-tools"));
    assert_eq!(open_commands(&world, &dir), [afkconvoy_core::harness::Harness::Pi.stock()]);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(world.claims().len(), 1);
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), before);
    let brief = brief(&dir);
    assert!(brief.contains("#70 is open"));
    assert!(brief.contains("unfinished.txt"));
    assert!(brief.contains("Fault evidence and pending work"));
    let plan = world.afk(&["plan", "--now", "1101"]).ok();
    assert_eq!(plan["host_kit"]["worker_health"]["workers"][0]["action"], "continued");
    assert_eq!(plan["host_kit"]["worker_health"]["workers"][0]["successor"], "pi");
    tick(&world, 1101);
    assert_eq!(std::fs::read_to_string(world.root.join("diagnosis-asked")).unwrap(), asked);
}

#[test]
fn ambiguous_normal_work_and_invalid_diagnoses_never_authorize_a_switch() {
    for (class, action) in [
        ("long_command", "wait"),
        ("waiting_human", "wait"),
        ("waiting_dependency", "wait"),
        ("complete", "complete"),
        ("unknown", "unknown"),
    ] {
        let world = blocked();
        let dir = worktree(&world, 7);
        diagnosis(&world, class, action, "worker", &[]);
        tick(&world, 800);
        tick(&world, 1100);
        assert_eq!(open_commands(&world, &dir), ["agent"], "{class}");
        assert_eq!(world.claims().len(), 1);
        assert!(world.root.join("diagnosis-asked").exists(), "{class}");
    }
    for answer in [
        "not JSON",
        r#"{"status":"fault","class":"stalled","scope":"model","evidence":["Your organization has disabled Claude Code access"],"action":"continue","missing":[],"retry_after":null}"#,
        r#"{"status":"fault","class":"quota","scope":"account","evidence":["invented"],"action":"continue","missing":[],"retry_after":null}"#,
    ] {
        let world = blocked();
        let dir = worktree(&world, 7);
        std::fs::write(world.root.join("diagnosis-answer"), answer).unwrap();
        tick(&world, 800);
        tick(&world, 1100);
        let asked = std::fs::read_to_string(world.root.join("diagnosis-asked")).unwrap();
        tick(&world, 1101);
        assert_eq!(std::fs::read_to_string(world.root.join("diagnosis-asked")).unwrap(), asked);
        assert_eq!(open_commands(&world, &dir), ["agent"]);
        assert_eq!(world.claims().len(), 1);
    }
}

#[test]
fn structured_rate_limits_wait_for_retry_after_before_a_bounded_session_fallback() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] =
                json!({"state":"idle", "error":{"code":"rate_limit_exceeded", "retry_after":120}});
        }
    });
    tick(&world, 800);
    tick(&world, 919);
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    let path = world.cache().join("afkconvoy/worker-health.json");
    let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(book["workers"][0]["rate_retries"], 1);
    assert_eq!(book["workers"][0]["next_check"], 920);
    tick(&world, 920);
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    tick(&world, 1040);
    assert_eq!(open_commands(&world, &dir), [afkconvoy_core::harness::Harness::Pi.stock()]);
    let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(book["faults"], json!([]), "a rate limit never pauses the entire account");
    assert!(
        !world.root.join("diagnosis-asked").exists(),
        "classified runtime errors need no model"
    );
}

#[test]
fn account_faults_continue_stalled_workers_across_checkouts_and_switch_all_is_host_scoped() {
    let world = World::new();
    let other = world.second_repo();
    world.issue(7, &["ready-for-agent"]);
    let mut gh = world.gh2();
    gh["issues"] = json!([{"number":8,"title":"second repo task", "labels":["ready-for-agent"], "state":"open"}]);
    std::fs::write(world.root.join("gh2.json"), gh.to_string()).unwrap();
    tick(&world, 700);
    world.afk_in(&other, &["tick", "--instance", "beta", "--host", "h1", "--now", "700"]).ok();
    world.hold_every_harness();
    world.say_concurrency(&world.clone, 3);
    world.say_concurrency(&other, 3);
    let (seven, eight) = (worktree(&world, 7), worktree(&world, 8));
    for dir in [&seven, &eight] {
        std::fs::write(dir.join("unfinished.txt"), "preserve").unwrap();
    }
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"idle", "error":{"code":"insufficient_quota"}});
        }
    });
    let kit = std::fs::read(world.kit_path()).unwrap();
    tick(&world, 800);
    world.afk_in(&other, &["tick", "--instance", "beta", "--host", "h1", "--now", "800"]).ok();
    for dir in [&seven, &eight] {
        assert_eq!(open_commands(&world, dir), [afkconvoy_core::harness::Harness::Pi.stock()]);
        assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "preserve");
    }
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
    let switched =
        world.afk_in(&world.root, &["switch", "--harness", "codex", "--all", "--host", "h1"]).ok();
    assert_eq!(switched["failed"], json!([]), "{switched}");
    assert_eq!(switched["switched"].as_array().unwrap().len(), 2);
    for dir in [&seven, &eight] {
        assert_eq!(open_commands(&world, dir), [codex_line(&world, dir)]);
    }
}

#[test]
fn a_model_fault_does_not_pause_other_models_or_interrupt_a_healthy_peer() {
    let world = World::new();
    world.kit_models("claude-code", &["opus", "haiku"]);
    world.issue(7, &["ready-for-agent", "harness:claude-code", "model:opus"]);
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:haiku"]);
    tick(&world, 700);
    world.hold_every_harness();
    let (seven, eight) = (worktree(&world, 7), worktree(&world, 8));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = if terminal["worktreePath"] == seven.to_str().unwrap() {
                json!({"state":"idle", "error":{"code":"model_unavailable"}})
            } else {
                json!({"state":"working", "stateStartedAt":790000})
            };
            terminal["lastOutputAt"] = json!(790000);
        }
    });
    tick(&world, 800);
    assert_eq!(open_commands(&world, &seven), [afkconvoy_core::harness::Harness::Pi.stock()]);
    assert_eq!(open_commands(&world, &eight), ["agent --model 'haiku'"]);
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let faults = &plan["selection"]["health"]["faults"];
    assert_eq!(faults[0]["scope"], "model");
    assert_eq!(faults[0]["model"], "opus");
    assert_eq!(world.paused(), json!([]));
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
}

#[test]
fn a_diagnosed_stuck_session_is_continued_without_pausing_its_harness() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!([
                "Agent session stopped accepting requests; the previous request never returned."
            ]);
        }
    });
    diagnosis(
        &world,
        "stalled",
        "continue",
        "worker",
        &["Agent session stopped accepting requests"],
    );
    tick(&world, 800);
    tick(&world, 1100);
    assert_eq!(open_commands(&world, &dir), [afkconvoy_core::harness::Harness::Pi.stock()]);
    let plan = world.afk(&["plan", "--now", "1101"]).ok();
    assert_eq!(plan["selection"]["health"]["faults"], json!([]));
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
}

#[test]
fn an_old_command_fault_does_not_disable_a_new_profile_configured_while_it_ran() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    world.hold_every_harness();
    let dir = worktree(&world, 7);
    world.kit_command("claude-code", "agent --new-profile");
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"idle", "error":{"code":"insufficient_quota"}});
        }
    });
    tick(&world, 800);
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    assert_eq!(plan["selection"]["eligible"][0], "claude-code", "{plan}");
    assert_eq!(open_commands(&world, &dir), ["agent --new-profile"]);
    assert_eq!(plan["selection"]["health"]["faults"][0]["scope"], "account");
}

#[test]
fn successful_recovery_restores_priority_for_new_work_without_moving_the_successor_back() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    world.hold_every_harness();
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"idle", "error":{"code":"insufficient_quota"}});
        }
    });
    tick(&world, 800);
    let successor = open_commands(&world, &dir);
    assert_eq!(successor, [afkconvoy_core::harness::Harness::Pi.stock()]);
    world.probes_answer();
    tick(&world, 4400);
    let plan = world.afk(&["plan", "--now", "4401"]).ok();
    assert_eq!(plan["selection"]["health"]["faults"], json!([]), "{plan}");
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
    assert_eq!(open_commands(&world, &dir), successor, "recovery never interrupts the successor");
    world.issue(8, &["ready-for-agent", "harness:claude-code"]);
    tick(&world, 4402);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), ["agent"]);
}

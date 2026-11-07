use crate::common;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::{World, codex_line, commit, git};
use serde_json::{Value, json};

fn hold_every_harness(world: &World) {
    world.hold_every_harness();
    world.kit_models("codex", &["gpt-x"]);
}

#[test]
fn switching_a_model_promotes_its_harness_and_preserves_every_entry() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_models("claude-code", &["opus", "claude-sonnet-5-5", "haiku"]);
    world.with_kit(|kit| {
        kit["harnesses"][0]["models"][1]["names"] = json!(["sonnet", "Sonnet 5.5"]);
        kit["harnesses"][0]["models"][1]["good_at"] = json!("wide changes");
    });
    world.afk(&["switch", "--harness", "codex"]).ok();
    let before: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    let moved = world.afk_in(&world.root, &["switch", "--model", "SONNET"]).ok();
    assert_eq!(moved["harness"], "claude-code");
    assert_eq!(moved["model"], "claude-sonnet-5-5");
    assert_eq!(moved["order"], json!(["claude-code", "codex", "pi", "dsh", "kimi", "grok"]));
    let after: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    assert_eq!(
        after["harnesses"][0]["models"],
        json!([
            before["harnesses"][1]["models"][1],
            before["harnesses"][1]["models"][0],
            before["harnesses"][1]["models"][2]
        ])
    );
    for at in 1..after["harnesses"].as_array().unwrap().len() {
        assert!(before["harnesses"].as_array().unwrap().contains(&after["harnesses"][at]));
    }
    assert_eq!(world.afk(&["switch", "--model", "Sonnet 5.5"]).ok()["changed"], false);
    assert_eq!(
        std::fs::read(world.kit_path()).unwrap(),
        serde_json::to_vec_pretty(&after).unwrap()
    );
}

#[test]
fn an_unknown_or_ambiguous_model_leaves_the_kit_unchanged_and_names_candidates() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_models("claude-code", &["claude-sonnet-5-5"]);
    world.kit_models("pi", &["anthropic/claude-sonnet-5-5"]);
    let before = std::fs::read(world.kit_path()).unwrap();
    let refused = world.afk(&["switch", "--model", "claude-sonnet-5-5"]).error();
    assert!(
        refused.contains("claude-code claude-sonnet-5-5")
            && refused.contains("pi anthropic/claude-sonnet-5-5"),
        "{refused}"
    );
    let unknown = world.afk(&["switch", "--model", "claude-sonnett-5-5"]).error();
    assert!(
        unknown.contains("nearest names") && unknown.contains("claude-sonnet-5-5"),
        "{unknown}"
    );
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), before);
}

#[test]
fn kit_priority_is_host_wide_idempotent_and_ignores_legacy_repository_choices() {
    let world = World::new();
    hold_every_harness(&world);
    world.kit_models("codex", &["gpt-x"]);
    let before: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    let other = world.second_repo();
    for (dir, old) in [(&world.clone, "pi"), (&other, "claude-code")] {
        let git_dir = PathBuf::from(git(dir, &["rev-parse", "--git-common-dir"]));
        let git_dir = if git_dir.is_absolute() { git_dir } else { dir.join(git_dir) };
        std::fs::write(git_dir.join("convoy-chosen-harness"), old).unwrap();
    }
    let moved = world.afk_in(&world.root, &["switch", "--harness", "codex"]).ok();
    assert_eq!(moved["order"], json!(["codex", "claude-code", "pi", "dsh", "kimi", "grok"]));
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
    assert_eq!(plan["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(plan["frontier"][0]["cast"]["why"], "user_default");
    assert!(world.afk(&["switch", "--clear"]).error().contains("--clear"));
    let back = world.afk_in(&other, &["switch", "--harness", "claude-code"]).ok();
    assert_eq!(back["order"], json!(["claude-code", "codex", "pi", "dsh", "kimi", "grok"]));
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

fn assert_guarded_harness(world: &World, dir: &Path, harness: afkconvoy_core::harness::Harness) {
    let commands = open_commands(world, dir);
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert!(command.contains(harness.stock()), "{command}");
    assert!(command.contains("AFKCONVOY_HARNESS="), "{command}");
    if harness == afkconvoy_core::harness::Harness::Pi {
        assert!(
            command.contains("AFKCONVOY_GUARD=") && command.contains("--extension"),
            "{command}"
        );
    } else {
        assert!(command.contains("KIMI_CODE_HOME="), "{command}");
    }
}

/// A world whose worker on issue 7 pushed its work, opened PR #70, and sits
/// at its prompt, its harness's access turned off.
fn blocked() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    // Every harness is held once the worker started: no caster held 7 back.
    hold_every_harness(&world);
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
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
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
        world.comments(70).into_iter().filter(|body| body.contains("convoy:landing")).collect();
    assert_eq!(records.len(), 1);
    assert!(records[0].starts_with("<!--convoy:landing switched=800/claude-code/codex/alpha-->"));
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
    assert_eq!(open_commands(&world, &eight), [common::trusted("agent")]);
    let records: Vec<String> =
        world.comments(80).into_iter().filter(|body| body.contains("convoy:landing")).collect();
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
fn a_missing_harness_starts_on_its_default_before_its_listing_is_researched() {
    let world = blocked();
    world.with_kit(|kit| {
        kit["harnesses"].as_array_mut().unwrap().retain(|held| held["harness"] != "kimi")
    });
    std::fs::write(world.root.join("shell-kimi-models"), r#"{"models":{"kimi-code/k3":{}}}"#)
        .unwrap();
    let dir = worktree(&world, 7);

    let started = std::time::Instant::now();
    let answer = switch(&world, "800", &["7", "--harness", "kimi"]).ok();
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(answer["added"], true);
    assert_eq!(answer["start_fault"], Value::Null);
    assert_eq!(answer["order"][0], "kimi");
    assert_eq!(answer["switched"][0]["to"], "kimi");
    assert_guarded_harness(&world, &dir, afkconvoy_core::harness::Harness::Kimi);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < until {
        let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
        if !kit["harnesses"][0]["models"].as_array().unwrap().is_empty() {
            assert_eq!(kit["harnesses"][0]["harness"], "kimi");
            assert_eq!(kit["harnesses"][0]["models"][0]["model"], "kimi-code/k3");
            assert_eq!(kit["harnesses"][0]["models"][0]["good_at"], "research pending");
            assert!(!world.root.join("research-asked").exists());
            assert!(
                !world
                    .shell_asked()
                    .iter()
                    .any(|asked| asked.contains("Reply with the single word OK."))
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("the targeted background refresh did not finish");
}

#[test]
fn an_added_harness_that_cannot_start_is_kept_and_reported_by_switch_and_doctor() {
    let world = World::new();
    world.logged_out("kimi");
    let answer = world.afk(&["switch", "--harness", "kimi"]).ok();
    assert_eq!(answer["added"], true);
    assert_eq!(answer["order"][0], "kimi");
    assert!(answer["start_fault"].as_str().unwrap().contains("kimi"));
    let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    assert_eq!(kit["harnesses"][0]["harness"], "kimi");
    assert_eq!(kit["harnesses"][0]["models"], json!([]));

    let doctor = world.afk(&["doctor", "--now", "800"]);
    let findings = doctor.json["findings"].to_string();
    assert!(findings.contains("kimi"), "{findings}");
}

#[test]
fn a_switch_names_every_skipped_harness_and_its_fix_in_the_answer_and_tick_log() {
    let world = World::new();
    world.hold_every_harness();
    world.with_kit(|kit| {
        kit["harnesses"].as_array_mut().unwrap().retain(|held| held["harness"] != "kimi")
    });
    world.repo(
        "acme/dev",
        &[
            (
                ".claude/skills/implement/SKILL.md",
                "---\nname: implement\ndescription: build\n---\n",
            ),
            (
                ".claude/settings.json",
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}"#,
            ),
        ],
    );
    world.repo(
        "acme/team",
        &[
            ("team.md", "```yaml\nroles:\n  dev: acme/dev\n```\n"),
            (
                "workflows/feature.md",
                "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    on: { pass: done }\n```\n",
            ),
        ],
    );
    common::write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n",
    );
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.issue(7, &["flow:feature"]);
    tick(&world, 700);

    let answer = switch(&world, "800", &["7", "--harness", "kimi"]).ok();
    let moved = &answer["switched"][0];
    assert_eq!(moved["to"], "claude-code");
    assert_eq!(moved["skipped"][0]["harness"], "kimi");
    assert_eq!(
        moved["skipped"][0]["why"],
        "role dev holds no limits for kimi (.kimi-code/limits.toml)"
    );
    assert_eq!(
        moved["skipped"][0]["fix"],
        "write the role's limits for kimi in its .kimi-code/limits.toml"
    );

    let runs = world.cache().join("afkconvoy/runs");
    let log = std::fs::read_dir(runs)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
        .unwrap();
    let entries: Vec<Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let logged = entries
        .iter()
        .find(|entry| entry["command"]["command"] == "switch")
        .expect("a switch tick-log line")["command"]["outcome"]
        .as_str()
        .unwrap();
    assert!(logged.contains("skipped kimi: role dev holds no limits for kimi"), "{logged}");
    assert!(logged.contains("to make it eligible: write the role's limits for kimi"), "{logged}");
}

#[test]
fn a_codex_successor_with_no_placed_model_starts_on_its_own_default() {
    let world = blocked();
    world.kit_models("codex", &[]);
    let dir = worktree(&world, 7);
    let answer = switch(&world, "800", &["7", "--harness", "codex"]).ok();
    assert_eq!(answer["switched"][0]["to"], "codex", "{answer}");
    let opened = open_commands(&world, &dir);
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(opened[0].contains("codex") && !opened[0].contains("--model"), "{opened:?}");
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn a_harness_that_cannot_be_started_is_refused_before_any_worker_is_ended() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.logged_out("codex");
    let said = switch(&world, "800", &["--all", "--harness", "codex"]).error();
    assert!(said.contains("the harness `codex` cannot be started here"), "{said}");
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert!(!brief(&dir).contains("You continue earlier work"));
    // nothing was chosen: the next start is on its casting still, which a
    // switch would have overruled
    world.issue(8, &["ready-for-agent", "bug", "harness:claude-code"]);
    tick(&world, 900);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), [common::trusted("agent")]);
}

#[test]
fn all_moves_every_claim_of_the_instance_and_a_claim_it_does_not_hold_is_refused() {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    world.issue(8, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    hold_every_harness(&world);
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
    assert_eq!(open_commands(&world, &seven), [common::trusted("agent")]);

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
    assert_eq!(open_commands(&world, &seven), [common::trusted("agent")]);
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
    let original_brief = brief(&dir);
    let original_claims = world.claims();
    world.control_termination_time();
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    let answer = switch(&world, "800", &["7", "--harness", "codex"]).ok();
    let error = answer["failed"][0]["error"].as_str().unwrap();
    assert!(error.contains("termination is unconfirmed"), "{error}");
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert_eq!(world.claims().len(), 1);
    assert_eq!(brief(&dir), original_brief, "an unconfirmed writer keeps its brief");
    assert_eq!(world.claims(), original_claims, "its claim stays held");
}

/// What `blocked()`'s worker shows.
const DISABLED: &str = "Your organization has disabled Claude Code access";

#[test]
fn screen_faults_are_judged_before_automatic_continuation_and_keep_the_scene() {
    let world = blocked();
    let dir = worktree(&world, 7);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    let before = std::fs::read(world.kit_path()).unwrap();
    world.jev_judges(&[("login_refused", 0.99)], DISABLED);
    tick(&world, 800);
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert!(!world.root.join("typesafe-request").exists());
    tick(&world, 1100);
    let asked = std::fs::read_to_string(world.root.join("typesafe-request")).unwrap();
    let payload = world.typesafe_payload();
    assert!(asked.contains("https://api.typesafe.ai/v1/systemone"));
    assert!(
        payload["state"]
            .as_str()
            .unwrap()
            .contains("Your organization has disabled Claude Code access")
    );
    assert_eq!(payload["questions"]["login_refused"]["type"], "noul");
    assert!(!world.asked_a_harness_about(DISABLED));
    assert_guarded_harness(&world, &dir, afkconvoy_core::harness::Harness::Pi);
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
    assert_eq!(std::fs::read_to_string(world.root.join("typesafe-request")).unwrap(), asked);
}

#[test]
fn ambiguous_normal_work_and_failed_judgments_never_authorize_a_switch() {
    for class in ["busy", "waiting_for_person", "trust_prompt", "finished", "unsettled"] {
        let world = blocked();
        let dir = worktree(&world, 7);
        let said = if class == "unsettled" { ("busy", 0.5) } else { (class, 0.99) };
        world.jev_judges(&[said], DISABLED);
        tick(&world, 800);
        tick(&world, 1100);
        assert_eq!(open_commands(&world, &dir), [common::trusted("agent")], "{class}");
        assert_eq!(world.claims().len(), 1);
        assert!(world.root.join("typesafe-request").exists(), "{class}");
        assert!(!world.asked_a_harness_about(DISABLED), "{class}");
    }
    for answer in ["not JSON", "{}"] {
        let world = blocked();
        let dir = worktree(&world, 7);
        world.jev_judges(&[], DISABLED);
        std::fs::write(world.root.join("typesafe-answer"), answer).unwrap();
        tick(&world, 800);
        tick(&world, 1100);
        let asked = std::fs::read_to_string(world.root.join("typesafe-request")).unwrap();
        tick(&world, 1101);
        assert_eq!(std::fs::read_to_string(world.root.join("typesafe-request")).unwrap(), asked);
        assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
        assert_eq!(world.claims().len(), 1);
        let health = world.afk(&["plan", "--now", "1101"]).ok();
        assert_eq!(
            health["host_kit"]["worker_health"]["workers"][0]["action"],
            "waiting for operator"
        );
    }
}

#[test]
fn structured_rate_limits_wait_for_retry_after_before_a_bounded_session_fallback() {
    let world = blocked();
    world.jev_key_command("printf unused-key");
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] =
                json!({"state":"idle", "error":{"code":"rate_limit_exceeded", "retry_after":120}});
        }
    });
    tick(&world, 800);
    tick(&world, 919);
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    let path = world.cache().join("afkconvoy/worker-health.json");
    let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(book["workers"][0]["rate_retries"], 1);
    assert_eq!(book["workers"][0]["next_check"], 920);
    tick(&world, 920);
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    tick(&world, 1040);
    assert_guarded_harness(&world, &dir, afkconvoy_core::harness::Harness::Pi);
    let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(book["faults"], json!([]), "a rate limit never pauses the entire account");
    assert!(!world.asked_a_harness_about(DISABLED), "classified runtime errors need no model");
    assert!(!world.root.join("typesafe-request").exists(), "the structured rule runs first");
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
    hold_every_harness(&world);
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
        assert_guarded_harness(&world, dir, afkconvoy_core::harness::Harness::Pi);
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
    hold_every_harness(&world);
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
    assert_eq!(open_commands(&world, &seven), [common::trusted("agent")]);
    assert_eq!(open_commands(&world, &eight), [common::trusted("agent --model 'haiku'")]);
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let faults = &plan["selection"]["health"]["faults"];
    assert_eq!(faults[0]["scope"], "model");
    assert_eq!(faults[0]["model"], "opus");
    assert_eq!(world.paused(), json!([]));
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
    let kit = std::fs::read(world.kit_path()).unwrap();
    world.issue(9, &["ready-for-agent"]);
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let nine = plan["frontier"].as_array().unwrap().iter().find(|i| i["number"] == 9).unwrap();
    assert_eq!(nine["cast"]["model"], Value::Null);
    assert_eq!(nine["cast"]["why"], "user_default");
    let status = plan["casting"].as_array().unwrap().iter().find(|s| s["number"] == 9).unwrap();
    assert_eq!(status["stages"][0]["skipped"], json!([]));
    world.probes_answer();
    tick(&world, 4400);
    world.issue(10, &["ready-for-agent"]);
    let plan = world.afk(&["plan", "--now", "4401"]).ok();
    let ten = plan["frontier"].as_array().unwrap().iter().find(|i| i["number"] == 10).unwrap();
    assert_eq!(ten["cast"]["model"], Value::Null);
    assert_eq!(ten["cast"]["why"], "user_default");
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
}

#[test]
fn switching_models_leaves_healthy_workers_alone_until_all_continues_them() {
    let world = World::new();
    world.kit_models("claude-code", &["opus", "haiku"]);
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    let dir = worktree(&world, 7);
    std::fs::write(dir.join("unfinished.txt"), "keep this work").unwrap();
    let claims = world.claims();
    let left = world.afk(&["switch", "--model", "haiku"]).ok();
    assert_eq!(left["switched"], json!([]), "{left}");
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    let moved = world.afk(&["switch", "--model", "haiku", "--all", "--host", "h1"]).ok();
    assert_eq!(moved["failed"], json!([]), "{moved}");
    assert_eq!(moved["switched"].as_array().unwrap().len(), 1, "{moved}");
    // The continuation restarts the worker and still does not take the model
    // `switch --model` moved first: no model label, so the user default.
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep this work");
    assert_eq!(world.claims(), claims);
}

#[test]
fn an_unclassed_stuck_session_waits_for_the_operator_without_pausing_its_harness() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!([
                "Agent session stopped accepting requests; the previous request never returned."
            ]);
        }
    });
    world.jev_judges(&[], "Agent session stopped accepting requests");
    tick(&world, 800);
    tick(&world, 1100);
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    let plan = world.afk(&["plan", "--now", "1101"]).ok();
    assert_eq!(plan["selection"]["health"]["faults"], json!([]));
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
    assert_eq!(plan["host_kit"]["worker_health"]["workers"][0]["action"], "waiting for operator");
}

#[test]
fn an_old_command_fault_does_not_disable_a_new_profile_configured_while_it_ran() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    hold_every_harness(&world);
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
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent --new-profile")]);
    assert_eq!(plan["selection"]["health"]["faults"][0]["scope"], "account");
}

#[test]
fn successful_recovery_restores_priority_for_new_work_without_moving_the_successor_back() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    hold_every_harness(&world);
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"idle", "error":{"code":"insufficient_quota"}});
        }
    });
    tick(&world, 800);
    let successor = open_commands(&world, &dir);
    assert_guarded_harness(&world, &dir, afkconvoy_core::harness::Harness::Pi);
    world.probes_answer();
    tick(&world, 4400);
    let plan = world.afk(&["plan", "--now", "4401"]).ok();
    assert_eq!(plan["selection"]["health"]["faults"], json!([]), "{plan}");
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
    assert_eq!(open_commands(&world, &dir), successor, "recovery never interrupts the successor");
    world.issue(8, &["ready-for-agent", "harness:claude-code"]);
    tick(&world, 4402);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), [common::trusted("agent")]);
}

fn codex_usage_limit(world: &World) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!([
                "■ Usage limit reached. You've reached your usage limit. Increase your limits to continue using codex.",
                "  Approaching rate limits",
                "  Switch to gpt-6-luna for lower credit usage?",
                "› 1. Switch to gpt-6-luna"
            ]);
            terminal["agent"] = json!({
                "state": "done", "outcome": "failure", "programStatus": {"state": "error"}
            });
        }
    });
}

// The refusal reported on hostvitals' sunfmin/control#98, 2026-10-26.
const CODEX_REFUSAL: &str = include_str!("screens/codex-task-refusal-control-98.txt");

// Codex's capacity screen reported on sunfmin/control#104, 2026-10-26.
const CODEX_CAPACITY: &str = include_str!("screens/codex-capacity-control-104.txt");

fn capacity_turn(world: &World, dir: &Path, state: &str, at: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == dir.to_str().unwrap() && terminal["open"] == true {
                terminal["screen"] = json!(CODEX_CAPACITY.lines().collect::<Vec<_>>());
                terminal["agent"] = json!({
                    "state":state, "outcome":"failure", "stateStartedAt":at * 1000,
                });
            }
        }
    });
}

fn health_book(world: &World) -> Value {
    serde_json::from_slice(
        &std::fs::read(world.cache().join("afkconvoy/worker-health.json")).unwrap(),
    )
    .unwrap()
}

fn capacity_nudges(world: &World, dir: &Path) -> usize {
    world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["worktreePath"] == dir.to_str().unwrap())
        .flat_map(|t| t["sent"].as_array().unwrap())
        .filter(|sent| {
            sent["text"].as_str().unwrap().starts_with("The selected model was at capacity.")
        })
        .count()
}

#[test]
fn the_capacity_rule_requires_codexs_observed_words_and_tolerates_wrapping() {
    use afkconvoy_core::harness::Harness;
    let screen =
        vec!["Selected model is at capacity. Please try a".into(), "different model.".into()];
    assert_eq!(Harness::Codex.capacity_stop(&screen).as_deref(), Some(CODEX_CAPACITY.trim()));
    assert!(Harness::Codex.capacity_stop(&["Selected model is at capacity.".into()]).is_none());
    assert!(Harness::ClaudeCode.capacity_stop(&screen).is_none());
}

#[test]
fn capacity_waits_and_nudges_twice_then_continues_without_faulting_the_harness() {
    let world = blocked();
    world.jev_key_command("printf unused-key");
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    let head = git(&dir, &["rev-parse", "HEAD"]);
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    let claims = world.claims();
    let kit = std::fs::read(world.kit_path()).unwrap();
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    capacity_turn(&world, &dir, "done", 790);
    tick(&world, 800);
    let book = health_book(&world);
    let worker = &book["workers"][0];
    assert_eq!(worker["source"], "capacity rule");
    assert_eq!(worker["diagnosis"]["class"], "capacity");
    assert_eq!(worker["diagnosis"]["scope"], "worker");
    assert_eq!(worker["judgment"], Value::Null);
    assert_eq!(worker["next_check"], 860);
    assert_eq!(worker["capacity"]["stops"], 1);
    assert_eq!(capacity_nudges(&world, &dir), 0);
    // Changed evidence during the wait must neither count again nor defer it.
    std::fs::write(dir.join("unfinished.txt"), "still keep me").unwrap();
    tick(&world, 859);
    assert_eq!(health_book(&world)["capacity_stops"]["codex"], 1);
    assert_eq!(capacity_nudges(&world, &dir), 0);
    tick(&world, 860);
    assert_eq!(capacity_nudges(&world, &dir), 1);
    assert_eq!(open_commands(&world, &dir), [codex_line(&world, &dir)]);
    // One working turn must not erase the episode's first stop.
    capacity_turn(&world, &dir, "working", 860);
    tick(&world, 861);
    capacity_turn(&world, &dir, "done", 862);
    tick(&world, 862);
    assert_eq!(health_book(&world)["workers"][0]["next_check"], 982);
    tick(&world, 981);
    assert_eq!(capacity_nudges(&world, &dir), 1);
    tick(&world, 982);
    assert_eq!(capacity_nudges(&world, &dir), 2);
    capacity_turn(&world, &dir, "done", 983);
    tick(&world, 983);
    let book = health_book(&world);
    assert_eq!(book["workers"][0]["action"], "continued");
    assert_eq!(book["workers"][0]["successor"], "claude-code");
    assert_eq!(book["capacity_stops"]["codex"], 3);
    assert_eq!(book["faults"], json!([]));
    assert!(!world.root.join("typesafe-request").exists());
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), branch);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "still keep me");
    assert_eq!(world.claims(), claims);
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
    assert!(brief(&dir).contains("#70 is open"));
    let doctor = world.afk(&["doctor", "--now", "984"]);
    assert_eq!(doctor.json["host"]["host_kit"]["worker_health"]["capacity_stops"]["codex"], 3);
    let logs = std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .map(|file| std::fs::read_to_string(file.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logs.contains("capacity rule"));
    assert!(logs.contains("waiting 60 seconds for capacity"));
    assert!(logs.contains("waiting 120 seconds for capacity"));
    assert!(logs.contains("capacity nudged"));
    world.issue(8, &["ready-for-agent"]);
    tick(&world, 984);
    assert_eq!(open_commands(&world, &worktree(&world, 8)).len(), 1);
    let plan = world.afk(&["plan", "--now", "985"]).ok();
    assert_eq!(plan["selection"]["refusals"]["7"], json!([]));
    assert!(open_commands(&world, &worktree(&world, 8))[0].contains("codex"));
}

#[test]
fn capacity_episode_ends_after_sustained_healthy_work_and_keeps_its_total() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    capacity_turn(&world, &dir, "done", 790);
    tick(&world, 800);
    tick(&world, 860);
    capacity_turn(&world, &dir, "working", 860);
    tick(&world, 861);
    tick(&world, 1161);
    assert!(health_book(&world)["workers"][0]["capacity"].is_null());
    capacity_turn(&world, &dir, "done", 1162);
    tick(&world, 1162);
    let book = health_book(&world);
    assert_eq!(book["workers"][0]["capacity"]["stops"], 1);
    assert_eq!(book["workers"][0]["next_check"], 1222);
    assert_eq!(book["capacity_stops"]["codex"], 2);
}

#[test]
fn capacity_never_spends_a_stop_on_an_input_that_started_no_turn() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    capacity_turn(&world, &dir, "done", 790);
    tick(&world, 800);
    world.with_orca(|orca| orca["prompt_stages"] = json!(["input_accepted"]));
    tick(&world, 860);
    tick(&world, 861);
    let book = health_book(&world);
    assert_eq!(book["workers"][0]["capacity"]["stops"], 1);
    assert_eq!(book["capacity_stops"]["codex"], 1);
    assert_eq!(book["workers"][0]["capacity"]["retry_at"], 860);
    assert_eq!(open_commands(&world, &dir), [codex_line(&world, &dir)]);
}

#[test]
fn capacity_continuation_preserves_the_stage_and_skips_harnesses_without_role_limits() {
    let world = World::new();
    hold_every_harness(&world);
    world.repo(
        "acme/dev",
        &[
            (
                ".claude/skills/implement/SKILL.md",
                "---\nname: implement\ndescription: build\n---\n",
            ),
            (".codex/hooks.json", r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}"#),
            (".pi/extensions/limits.js", "export default () => {};\n"),
        ],
    );
    world.repo("acme/team", &[
        ("team.md", "```yaml\nroles:\n  dev: acme/dev\n```\n"),
        ("workflows/feature.md", "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    on: { pass: done }\n```\n"),
    ]);
    common::write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n",
    );
    git(&world.clone, &["commit", "-qam", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.afk(&["switch", "--harness", "codex"]).ok();
    world.issue(7, &["flow:feature"]);
    tick(&world, 700);
    let dir = worktree(&world, 7);
    let claims = world.claims();
    capacity_turn(&world, &dir, "done", 790);
    for at in [800, 860, 861, 981, 982] {
        tick(&world, at);
    }
    assert_eq!(health_book(&world)["workers"][0]["successor"], "pi");
    assert_eq!(world.afk(&["show", "--issue", "7"]).ok()["position"]["stage"], "build");
    assert_eq!(world.claims(), claims);
    assert_eq!(open_commands(&world, &dir).len(), 1);
    assert!(open_commands(&world, &dir)[0].contains("pi"));
}

#[test]
fn capacity_continuation_never_starts_a_second_writer_before_termination_is_confirmed() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    capacity_turn(&world, &dir, "done", 790);
    for at in [800, 860, 861, 981] {
        tick(&world, at);
    }
    let commands = open_commands(&world, &dir);
    let original_brief = brief(&dir);
    let original_claims = world.claims();
    world.control_termination_time();
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    tick(&world, 982);
    assert_eq!(open_commands(&world, &dir), commands);
    assert_eq!(world.claims().len(), 1);
    assert!(
        health_book(&world)["workers"][0]["error"]
            .as_str()
            .unwrap()
            .contains("termination is unconfirmed")
    );
    assert_eq!(health_book(&world)["faults"], json!([]));
    assert_eq!(brief(&dir), original_brief, "an unconfirmed writer keeps its brief");
    assert_eq!(world.claims(), original_claims, "its claim stays held");
}

fn codex_refusal(world: &World, dir: &Path) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == dir.to_str().unwrap() && terminal["open"] == true {
                terminal["screen"] = json!(CODEX_REFUSAL.lines().collect::<Vec<_>>());
                // A refused turn is stopped; Orca remains authoritative for working turns.
                terminal["agent"] =
                    json!({"state":"done", "outcome":"failure", "stateStartedAt":790000});
            }
        }
    });
}

#[test]
fn the_task_refusal_rule_requires_the_observed_harness_words_and_tolerates_wrapping() {
    use afkconvoy_core::harness::Harness;
    let wrapped: Vec<String> = CODEX_REFUSAL.split(". ").map(str::to_string).collect();
    let screen = vec![
        "We take extra care with some cybersecurity requests.".into(),
        "If you're doing authorized security work, apply for Daybreak".into(),
        "to get broader access.".into(),
    ];
    assert_eq!(Harness::Codex.task_refusal(&screen).as_deref(), Some(CODEX_REFUSAL.trim()));
    assert!(Harness::Codex.task_refusal(&wrapped).is_none());
    assert!(Harness::Codex.task_refusal(&["I cannot help with that request.".into()]).is_none());
    assert!(Harness::ClaudeCode.task_refusal(&screen).is_none());
}

#[test]
fn codex_task_refusal_continues_immediately_and_only_excludes_this_issue() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    let head = git(&dir, &["rev-parse", "HEAD"]);
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    let claims = world.claims();
    let kit = std::fs::read(world.kit_path()).unwrap();
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    codex_refusal(&world, &dir);
    std::fs::write(world.cache().join("afkconvoy/worker-health.json"), json!({"workers":[], "faults":[], "observations_failed":[[dir,"a previous failed observation",2000]]}).to_string()).unwrap();

    tick(&world, 800);

    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), branch);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
    assert_eq!(world.claims(), claims);
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
    assert!(!world.root.join("typesafe-request").exists());
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let worker = &plan["selection"]["health"]["workers"][0];
    assert_eq!(worker["source"], "terminal rule");
    assert_eq!(worker["diagnosis"]["class"], "refusal");
    assert_eq!(worker["action"], "continued");
    assert_eq!(worker["judgment"], Value::Null);
    assert_eq!(plan["selection"]["health"]["faults"], json!([]));
    assert_eq!(plan["selection"]["refusals"]["7"][0]["harness"], "codex");
    assert!(
        world
            .comments(70)
            .iter()
            .any(|body| body.contains("switched from `codex` to `claude-code`"))
    );
    let doctor = world.afk(&["doctor", "--now", "801"]);
    assert_eq!(doctor.json["refusals"]["7"][0]["evidence"], CODEX_REFUSAL.trim());

    // A fresh CLI process has no health observation left to lean on.
    std::fs::remove_file(world.cache().join("afkconvoy/worker-health.json")).unwrap();
    let moved = switch(&world, "802", &["7", "--harness", "codex"]).ok();
    assert_eq!(moved["switched"][0]["to"], "claude-code");
    assert!(
        moved["switched"][0]["skipped"][0]["why"].as_str().unwrap().contains("refused this task")
    );
    assert_eq!(
        world.comments(7).iter().filter(|body| body.starts_with("<!--convoy:refusal ")).count(),
        1
    );
    world.issue(8, &["ready-for-agent"]);
    tick(&world, 803);
    assert_eq!(
        open_commands(&world, &worktree(&world, 8)),
        [codex_line(&world, &worktree(&world, 8))]
    );
}

#[test]
fn codex_task_refusal_never_starts_a_second_writer_while_termination_is_unconfirmed() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    codex_refusal(&world, &dir);
    let original_brief = brief(&dir);
    let original_claims = world.claims();
    world.control_termination_time();
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    tick(&world, 800);
    assert_eq!(open_commands(&world, &dir), [codex_line(&world, &dir)]);
    assert_eq!(world.claims().len(), 1);
    assert!(!world.root.join("typesafe-request").exists());
    assert!(world.comments(7).iter().any(|body| body.starts_with("<!--convoy:refusal ")));
    assert_eq!(brief(&dir), original_brief, "an unconfirmed writer keeps its brief");
    assert_eq!(world.claims(), original_claims, "its claim stays held");
}

#[test]
fn codex_task_refusal_with_no_harness_left_escalates_now_and_stays_with_the_human() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    world.with_kit(|kit| {
        kit["harnesses"].as_array_mut().unwrap().retain(|h| h["harness"] == "codex")
    });
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    codex_refusal(&world, &dir);
    tick(&world, 800);
    assert!(world.claims().is_empty());
    assert!(open_commands(&world, &dir).is_empty());
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
    let escalation = world
        .comments(7)
        .into_iter()
        .find(|body| body.starts_with("<!--convoy:escalation "))
        .unwrap();
    assert!(
        escalation.contains("refusal=exhausted")
            && escalation.contains("No eligible harness remains")
            && escalation.contains(CODEX_REFUSAL.trim()),
        "{escalation}"
    );
    assert!(!escalation.contains("one hour"));
    tick(&world, 801);
    let plan = world.afk(&["plan", "--now", "802"]).ok();
    assert_eq!(plan["frontier"], json!([]));
    assert!(!world.comments(7).iter().any(|body| body.starts_with("<!--convoy:consult ")));
    assert!(!world.root.join("typesafe-request").exists());
}

#[test]
fn codex_task_refusal_escalates_when_the_remaining_harness_is_paused() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    world.with_kit(|kit| {
        kit["harnesses"]
            .as_array_mut()
            .unwrap()
            .retain(|h| h["harness"] == "codex" || h["harness"] == "claude-code")
    });
    world.pause("claude-code", 790, "no allowance");
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    codex_refusal(&world, &dir);
    tick(&world, 800);
    assert!(world.claims().is_empty());
    assert!(
        world
            .comments(7)
            .iter()
            .any(|body| body.contains("refusal=exhausted") && body.contains(CODEX_REFUSAL.trim()))
    );
    assert!(open_commands(&world, &dir).is_empty());
    tick(&world, 801);
    assert!(!world.comments(7).iter().any(|body| body.starts_with("<!--convoy:consult ")));
}

#[test]
fn an_issue_refusal_is_remembered_at_the_next_stage_and_shown_without_host_health() {
    use afkconvoy_core::harness::Harness;
    let world = World::new();
    hold_every_harness(&world);
    world.repo(
        "acme/dev",
        &[
            (
                ".claude/skills/implement/SKILL.md",
                "---\nname: implement\ndescription: build\n---\n",
            ),
            (".codex/hooks.json", r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}"#),
            (".pi/extensions/limits.js", "export default () => {};\n"),
        ],
    );
    world.repo("acme/team", &[
        ("team.md", "```yaml\nroles:\n  dev: acme/dev\n```\n"),
        ("workflows/feature.md", "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    on: { pass: review }\n  - name: review\n    by: dev\n    do: implement\n    writes: false\n    on: { pass: done }\n```\n"),
    ]);
    common::write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n",
    );
    git(&world.clone, &["commit", "-qam", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.afk(&["switch", "--harness", "codex"]).ok();
    world.issue(7, &["flow:feature"]);
    tick(&world, 700);
    let dir = worktree(&world, 7);
    codex_refusal(&world, &dir);
    tick(&world, 800);
    assert_eq!(world.afk(&["show", "--issue", "7"]).ok()["position"]["stage"], "build");
    world
        .afk(&[
            "verdict",
            "pass",
            "--issue",
            "7",
            "--stage",
            "build",
            "--role-commit",
            "r1",
            "--team-commit",
            "t1",
            "--text",
            "done",
        ])
        .ok();
    tick(&world, 801);
    let shown = world.afk(&["show", "--issue", "7"]).ok();
    assert_eq!(shown["position"]["stage"], "review");
    assert_eq!(shown["refusals"][0]["harness"], "codex");
    assert_eq!(shown["refusals"][0]["evidence"], CODEX_REFUSAL.trim());
    let plan = world.afk(&["plan", "--now", "802"]).ok();
    let casting = &plan["casting"][0];
    assert!(
        casting["stages"].as_array().unwrap().iter().all(|s| s["harness"] == "pi"),
        "{casting}"
    );
    assert_eq!(
        std::fs::read_to_string(
            PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"])).join("convoy-harness")
        )
        .unwrap()
        .trim(),
        Harness::Pi.key()
    );
}

#[test]
fn codex_explicit_usage_limit_continues_by_rule_without_asking_jev() {
    let world = blocked();
    world.jev_key_command("printf unused-key");
    let dir = worktree(&world, 7);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    codex_usage_limit(&world);

    tick(&world, 800);

    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
    assert_eq!(world.claims().len(), 1);
    assert!(!world.asked_a_harness_about("Usage limit reached"));
    assert!(!world.root.join("typesafe-request").exists(), "the terminal rule runs first");
    let health = world.afk(&["plan", "--now", "801"]).ok();
    let worker = &health["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["source"], "terminal rule");
    assert_eq!(worker["diagnosis"]["class"], "quota");
    assert_eq!(worker["diagnosis"]["scope"], "account");
    assert_eq!(worker["action"], "continued");
    assert!(
        world
            .comments(70)
            .iter()
            .any(|body| body.contains("codex") && body.contains("claude-code"))
    );
}

const ZAI_SCREEN: &str = include_str!("screens/pi-zai-usage-limit-control-163.txt");
const ZAI_RUNTIME: &str = r#"429: {"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at 2026-10-27 17:52:40"}"#;

#[test]
fn pi_zai_usage_limit_is_an_account_quota_by_rule_from_either_source() {
    for (source, rule, evidence) in [
        ("terminal", "terminal rule", ZAI_SCREEN.lines().next().unwrap()),
        ("runtime", "structured rule", ZAI_RUNTIME),
    ] {
        let world = blocked();
        world.jev_key_command("printf unused-key");
        world.kit_models("pi", &["zai/glm-5.3"]);
        let dir = worktree(&world, 7);
        switch(&world, "750", &["7", "--harness", "pi"]).ok();
        let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
        let claims = world.claims();
        let pr = world.gh()["prs"][0].clone();
        let kit = std::fs::read(world.kit_path()).unwrap();
        std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                terminal["agent"] = json!({
                    "state": "done", "outcome": "failure",
                    "programStatus": {"state": "error"}
                });
                if source == "terminal" {
                    terminal["screen"] = json!(ZAI_SCREEN.lines().collect::<Vec<_>>());
                } else {
                    terminal["screen"] = json!(["pi stopped"]);
                    terminal["agent"]["programStatus"]["message"] = json!(ZAI_RUNTIME);
                }
            }
        });

        tick(&world, 800);
        world.issue(8, &["ready-for-agent"]);

        assert_eq!(open_commands(&world, &dir), [common::trusted("agent")], "{source}");
        assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), branch);
        assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
        assert_eq!(world.claims(), claims);
        assert_eq!(world.gh()["prs"][0], pr);
        assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
        assert!(!world.root.join("typesafe-request").exists(), "{source} rule runs first");
        let plan = world.afk(&["plan", "--now", "801"]).ok();
        let health = &plan["host_kit"]["worker_health"];
        let worker = &health["workers"][0];
        assert_eq!(worker["source"], rule);
        assert_eq!(worker["diagnosis"]["class"], "quota", "{source}: {worker}");
        assert_eq!(worker["diagnosis"]["scope"], "account");
        assert_eq!(
            worker["diagnosis"]["retry_after"],
            Value::Null,
            "the reset has no provider zone"
        );
        assert_eq!(worker["action"], "continued");
        assert_eq!(worker["successor"], "claude-code");
        assert_eq!(health["faults"][0]["harness"], "pi");
        assert_eq!(health["faults"][0]["scope"], "account");
        assert_eq!(health["faults"][0]["evidence"][0], evidence);
        assert_eq!(plan["selection"]["order"][0], "pi");
        assert_eq!(plan["selection"]["eligible"][0], "claude-code");
        let skipped = switch(&world, "801", &["7", "--harness", "pi"]).ok();
        assert_eq!(skipped["switched"][0]["to"], "claude-code");
        let why = skipped["switched"][0]["skipped"][0]["why"].as_str().unwrap();
        assert!(why.contains("pi has an account fault") && why.contains(evidence), "{why}");

        world.say_concurrency(&world.clone, 2);
        tick(&world, 802);
        assert_eq!(open_commands(&world, &worktree(&world, 8)), [common::trusted("agent")]);

        world.probes_answer();
        tick(&world, 4400);
        let recovered = world.afk(&["plan", "--now", "4401"]).ok();
        assert_eq!(recovered["selection"]["health"]["faults"], json!([]));
        assert_eq!(recovered["selection"]["eligible"][0], "pi");
    }
}

const KIMI_SCREEN: &str = include_str!("screens/kimi-5-hour-usage-limit-control-166.txt");

#[test]
fn kimi_5_hour_usage_limit_is_an_account_quota_by_rule_and_continues() {
    let world = blocked();
    world.jev_key_command("printf unused-key");
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "kimi"]).ok();
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    let claims = world.claims();
    let pr = world.gh()["prs"][0].clone();
    let kit = std::fs::read(world.kit_path()).unwrap();
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    let evidence = KIMI_SCREEN.lines().next().unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({
                "state": "done", "outcome": "failure",
                "programStatus": {"state": "error"}
            });
            terminal["screen"] = json!(KIMI_SCREEN.lines().collect::<Vec<_>>());
        }
    });

    tick(&world, 800);
    world.issue(8, &["ready-for-agent"]);

    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), branch);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
    assert_eq!(world.claims(), claims);
    assert_eq!(world.gh()["prs"][0], pr);
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
    assert!(!world.root.join("typesafe-request").exists(), "the terminal rule runs first");
    assert!(!world.asked_a_harness_about("5-hour usage limit"));
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let health = &plan["host_kit"]["worker_health"];
    let worker = &health["workers"][0];
    assert_eq!(worker["source"], "terminal rule");
    assert_eq!(worker["diagnosis"]["class"], "quota", "{worker}");
    assert_eq!(worker["diagnosis"]["scope"], "account");
    assert_eq!(worker["diagnosis"]["retry_after"], Value::Null);
    assert_eq!(worker["action"], "continued");
    assert_eq!(worker["successor"], "claude-code");
    assert_eq!(health["faults"][0]["harness"], "kimi");
    assert_eq!(health["faults"][0]["scope"], "account");
    assert_eq!(health["faults"][0]["evidence"][0], evidence);
    assert_eq!(plan["selection"]["order"][0], "kimi");
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");
    let skipped = switch(&world, "801", &["7", "--harness", "kimi"]).ok();
    assert_eq!(skipped["switched"][0]["to"], "claude-code");
    let why = skipped["switched"][0]["skipped"][0]["why"].as_str().unwrap();
    assert!(why.contains("kimi has an account fault") && why.contains(evidence), "{why}");

    world.say_concurrency(&world.clone, 2);
    tick(&world, 802);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), [common::trusted("agent")]);

    world.probes_answer();
    tick(&world, 4400);
    let recovered = world.afk(&["plan", "--now", "4401"]).ok();
    assert_eq!(recovered["selection"]["health"]["faults"], json!([]));
    assert_eq!(recovered["selection"]["eligible"][0], "kimi");
}

#[test]
fn kimi_usage_limit_waits_out_a_running_gate_then_continues() {
    let world = blocked();
    world.jev_key_command("printf unused-key");
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "kimi"]).ok();
    let kimi = open_commands(&world, &dir);
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    let claims = world.claims();
    let pr = world.gh()["prs"][0].clone();
    let kit = std::fs::read(world.kit_path()).unwrap();
    std::fs::write(dir.join("unfinished.txt"), "keep me").unwrap();
    let evidence = KIMI_SCREEN.lines().next().unwrap();
    let mut lines: Vec<String> = KIMI_SCREEN.lines().map(str::to_string).collect();
    lines.push("❯".into());
    lines.push("kimi-code/k3.1-pro  [1 task running]".into());
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({
                "state": "done", "outcome": "failure",
                "programStatus": {"state": "error"}
            });
            terminal["screen"] = json!(lines.clone());
        }
    });

    tick(&world, 800);

    assert_eq!(open_commands(&world, &dir), kimi, "a running gate is not cut short");
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
    assert!(!world.root.join("typesafe-request").exists());
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let health = &plan["host_kit"]["worker_health"];
    let worker = &health["workers"][0];
    assert_eq!(worker["source"], "terminal rule");
    assert_eq!(worker["diagnosis"]["class"], "quota");
    assert_eq!(worker["diagnosis"]["scope"], "account");
    assert_eq!(worker["action"], "waiting for background work");
    assert_eq!(worker["successor"], Value::Null);
    assert_eq!(health["faults"][0]["harness"], "kimi");
    assert_eq!(health["faults"][0]["evidence"][0], evidence);
    assert_eq!(plan["selection"]["order"][0], "kimi");
    assert_eq!(plan["selection"]["eligible"][0], "claude-code");

    world.issue(8, &["ready-for-agent"]);
    world.say_concurrency(&world.clone, 2);
    tick(&world, 802);
    assert_eq!(open_commands(&world, &dir), kimi);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), [common::trusted("agent")]);

    lines.pop();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == dir.to_str().unwrap() {
                terminal["screen"] = json!(lines);
            }
        }
    });
    tick(&world, 900);

    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), branch);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "keep me");
    assert!(world.claims().contains(&claims[0]), "issue 7 keeps its claim");
    assert_eq!(world.gh()["prs"][0], pr);
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
    let continued = world.afk(&["plan", "--now", "901"]).ok();
    let worker = &continued["host_kit"]["worker_health"]["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["action"] == "continued")
        .expect("the stopped worker continued");
    assert_eq!(worker["successor"], "claude-code");
    assert_eq!(continued["selection"]["order"][0], "kimi");
}

#[test]
fn explicit_usage_limit_starts_nothing_until_the_failed_codex_agent_is_gone() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    codex_usage_limit(&world);
    let original_brief = brief(&dir);
    let original_claims = world.claims();
    world.control_termination_time();
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));

    tick(&world, 800);

    assert_eq!(open_commands(&world, &dir), [codex_line(&world, &dir)]);
    let health = world.afk(&["plan", "--now", "801"]).ok();
    let worker = &health["host_kit"]["worker_health"]["workers"][0];
    assert!(worker["error"].as_str().unwrap().contains("termination is unconfirmed"), "{worker}");
    assert_eq!(worker["successor"], Value::Null);
    assert_eq!(world.claims().len(), 1);
    assert_eq!(brief(&dir), original_brief, "an unconfirmed writer keeps its brief");
    assert_eq!(world.claims(), original_claims, "its claim stays held");
}

#[test]
fn an_account_quota_jev_judges_without_a_reset_continues_at_once() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!(["Usage limit reached. Increase your limits to continue."]);
        }
    });
    world.jev_judges(
        &[("usage_limit", 0.99)],
        "Usage limit reached. Increase your limits to continue.",
    );

    tick(&world, 800);
    tick(&world, 1100);

    assert_guarded_harness(&world, &dir, afkconvoy_core::harness::Harness::Pi);
    let health = world.afk(&["plan", "--now", "1101"]).ok();
    let worker = &health["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "continued");
    assert_eq!(worker["source"], "jev");
    assert_eq!(worker["diagnosis"]["retry_after"], Value::Null);
}

#[test]
fn a_failed_typesafe_call_waits_for_the_operator_and_is_logged() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.jev_judges(&[("usage_limit", 0.99)], DISABLED);
    std::fs::write(world.root.join("typesafe-fail"), "refused by the recorded endpoint").unwrap();

    tick(&world, 800);
    tick(&world, 1100);

    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert!(!world.asked_a_harness_about(DISABLED));
    let path = world.cache().join("afkconvoy/worker-health.json");
    let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let worker = &book["workers"][0];
    assert_eq!(worker["action"], "waiting for operator");
    assert!(worker["error"].as_str().unwrap().contains("TypeSafe request failed"));
    assert_eq!(worker["judgment"]["request"]["model"], "jev-latest");
    let logs = std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logs.contains("TypeSafe request failed") && logs.contains("\"failed\":true"), "{logs}");
}

/// The fields of the failed Codex agent reported in sunfmin/control#83.
/// Pane identity and times are fixed here; mainAgent owns the outcome.
fn failed_agent() -> Value {
    serde_json::from_str(include_str!("fixtures/orca-failed-agent.json")).unwrap()
}

#[test]
fn orcas_failed_turn_is_classified_on_its_first_observation() {
    for failure in [false, true] {
        let world = blocked();
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                terminal["agent"] = failed_agent();
                if !failure {
                    terminal["agent"]["mainAgent"]["outcome"] = json!("success");
                }
                terminal["screen"] = json!(["The session ended unexpectedly"]);
            }
        });
        world.jev_judges(&[], "The session ended unexpectedly");
        tick(&world, 800);
        assert_eq!(world.root.join("typesafe-request").exists(), failure);
        let health = world.afk(&["plan", "--now", "801"]).ok();
        let worker = &health["host_kit"]["worker_health"]["workers"][0];
        assert_eq!(worker["source"] == "jev", failure, "{worker}");
        assert_eq!(worker["orca"]["state"], "error");
        assert_eq!(worker["orca"]["agentType"], "codex");
        assert_eq!(worker["orca"]["stateStartedAt"], 799000);
    }
}

#[test]
fn working_agents_are_healthy_without_screen_judgment_and_share_one_tick_read() {
    let world = fleet_with_two_workers();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"agentType":"claude", "state":"working",
                "stateStartedAt":799000, "mainAgent":{"state":"working"}});
            terminal["lastOutputAt"] = json!(799000);
            terminal["screen"] = json!(["Old error: Usage limit reached"]);
        }
    });
    world.jev_key_command("printf test-key");
    world.orca_calls();
    tick(&world, 800);
    let calls = world.orca_calls();
    assert_eq!(calls.iter().filter(|c| common::asked(c) == "worktree ps").count(), 1);
    assert!(!calls.iter().any(|c| common::asked(c) == "terminal read"), "{calls:?}");
    let health = world.afk(&["plan", "--now", "801"]).ok();
    let workers = health["host_kit"]["worker_health"]["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 2);
    assert!(workers.iter().all(|w| w["action"] == "healthy" && w["orca"]["state"] == "working"));
    assert!(
        workers.iter().all(|w| w["next_check"].as_u64().unwrap() > 801),
        "a just-observed healthy worker must not immediately look overdue to the watch: {workers:?}"
    );
    assert!(!world.root.join("typesafe-request").exists());
}

#[test]
fn a_worker_that_stops_is_observed_before_its_healthy_check_deadline() {
    let world = fleet_with_two_workers();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"agentType":"claude", "state":"working",
                "stateStartedAt":799000});
        }
    });
    tick(&world, 800);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"agentType":"claude", "state":"done",
                "stateStartedAt":809000, "mainAgent":{"outcome":"success"}});
            terminal["lastOutputAt"] = json!(809000);
            terminal["screen"] = json!(["finished the work"]);
        }
    });
    tick(&world, 810);
    let health = world.afk(&["plan", "--now", "811"]).ok();
    let workers = health["host_kit"]["worker_health"]["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 2);
    assert!(
        workers.iter().all(|w| w["action"] == "observing" && w["orca"]["state"] == "done"),
        "{workers:?}"
    );
}

fn fleet_with_two_workers() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    world.say_concurrency(&world.clone, 2);
    tick(&world, 700);
    world
}

#[test]
fn the_live_runners_tick_and_served_panel_share_one_orca_observation() {
    let world = fleet_with_two_workers();
    // A watch starting in the runner has its own live input check, on a
    // separate thread with this same parent PID. Keep its independent look
    // within its interval so the count measures only the tick and panel.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    common::write(
        &world.cache(),
        "afkconvoy/watch/state.json",
        &json!({"looked": now}).to_string(),
    );
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"working", "agentType":"claude",
                "stateStartedAt":799000});
            terminal["lastOutputAt"] = json!(799000);
        }
    });
    world.orca_calls();
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    command.args([
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        "800",
        "--ticks",
        "1",
        "--second-ms",
        "0",
    ]);
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let runner = child.id();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let orca = world.orca();
    let calls: Vec<Vec<String>> = serde_json::from_value(orca["calls"].clone()).unwrap();
    let parents: Vec<u32> = serde_json::from_value(orca["parents"].clone()).unwrap();
    assert_eq!(calls.len(), parents.len());
    // This fleet's tick and panel share exactly one observation.
    let observations = calls
        .iter()
        .zip(parents)
        .filter(|(call, parent)| *parent == runner && common::asked(call) == "worktree ps")
        .count();
    assert_eq!(observations, 1, "{calls:?}");
}

#[test]
fn a_failed_delivery_is_classified_even_if_orcas_last_report_was_working() {
    let world = fleet_with_two_workers();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"working", "agentType":"claude",
                "stateStartedAt":700000});
        }
    });
    tick(&world, 750);
    let dir = worktree(&world, 7);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::write(
        notes.join("convoy-worker-error.json"),
        json!({
            "source":"orca terminal send", "error":{"code":"insufficient_quota"}, "at":799
        })
        .to_string(),
    )
    .unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"working", "agentType":"claude",
                "stateStartedAt":700000});
            terminal["lastOutputAt"] = json!(799000);
        }
    });
    world.jev_key_command("printf test-key");
    tick(&world, 800);
    let health = world.afk(&["plan", "--now", "801"]).ok();
    let workers = health["host_kit"]["worker_health"]["workers"].as_array().unwrap();
    let worker = workers.iter().find(|w| w["path"] == json!(dir)).unwrap();
    assert_eq!(worker["orca"]["state"], "working");
    assert_eq!(worker["diagnosis"]["class"], "quota");
    assert_eq!(worker["source"], "structured rule");
    assert_ne!(worker["action"], "healthy");
    assert!(!world.root.join("typesafe-request").exists());
}

#[test]
fn orcas_harness_overrules_a_wrong_record_for_quota_recovery_and_is_shown() {
    let world = blocked();
    let dir = worktree(&world, 7);
    switch(&world, "750", &["7", "--harness", "codex"]).ok();
    codex_usage_limit(&world);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = failed_agent();
        }
    });
    let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::write(git_dir.join("convoy-harness"), "claude-code").unwrap();
    world.jev_key_command("printf test-key");
    tick(&world, 800);
    assert_eq!(open_commands(&world, &dir), [common::trusted("agent")]);
    assert!(!world.root.join("typesafe-request").exists());
    let health = world.afk(&["doctor", "--now", "801"]);
    // Recovery ran in a standalone tick; no host runner holds the claim.
    assert_eq!(health.code, 1);
    let health = health.json;
    assert!(health["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "no_run"));
    let health = &health["host"]["host_kit"]["worker_health"];
    assert_eq!(health["workers"][0]["harness"], "codex");
    assert_eq!(health["workers"][0]["recorded_harness"], "claude-code");
    assert_eq!(health["workers"][0]["harness_mismatch"], true);
    assert_eq!(health["faults"][0]["harness"], "codex");
    let panel = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["panel", "--host", "h1", "--now", "801"])
        .output()
        .unwrap();
    assert!(panel.status.success());
    let html = String::from_utf8(panel.stdout).unwrap();
    assert!(html.contains("failure") && html.contains("codex") && html.contains("799000"));
    assert!(html.contains("recorded claude-code"));
}

#[test]
fn switching_to_a_removed_harness_restores_it_first_and_clears_the_removal() {
    let world = World::new();
    hold_every_harness(&world);
    world.shell_models(&["provider/model"]);
    world.afk(&["kit", "--remove", "pi", "--now", "700"]).ok();

    let restored = switch(&world, "800", &["--harness", "pi"]).ok();
    assert_eq!(restored["restored"], true);
    assert_eq!(restored["added"], true);
    assert_eq!(restored["start_fault"], Value::Null);
    assert_eq!(restored["order"][0], "pi");
    let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    assert!(kit.get("removed").is_none(), "{kit}");
    assert_eq!(kit["harnesses"][0]["harness"], "pi");

    let again = switch(&world, "801", &["--harness", "pi"]).ok();
    assert_eq!(again["changed"], false);
    assert_eq!(again["restored"], false);
}

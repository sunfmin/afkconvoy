use crate::common;

use std::path::Path;

use common::{BASE, World, git};
use serde_json::{Value, json};

const COMMAND: &str = "ckimi --flag 'quoted arg'";

fn dispatch(world: &World, issue: &str) -> common::Answer {
    world.kit_command("claude-code", COMMAND);
    world.afk(&[
        "dispatch",
        "--issue",
        issue,
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        "1000",
    ])
}

fn ready_world() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.with_gh(|gh| gh["issues"][0]["title"] = json!("Add the `plan` command"));
    world
}

#[test]
fn a_brief_with_lone_surrogates_in_the_screen_and_receipt_is_judged_on_the_receipt() {
    for started in [false, true] {
        let world = ready_world();
        let stages = if started {
            json!(["input_accepted", "turn_started"])
        } else {
            json!(["input_accepted"])
        };
        let receipt = json!({"ok": true, "result": {"send": {"accepted": true,
            "prompt": {"stages": stages, "observation": "⏺SURROGATE"}}}})
        .to_string()
        .replace("SURROGATE", r"\udf17");
        world.with_orca(|orca| {
            orca["raw_answers"] = json!({
                "terminal wait": r#"{"ok":true,"result":{"wait":{"satisfied":true,"preview":"⏺\ud800"}}}"#,
                "terminal send": receipt,
                "terminal read": r#"{"ok":true,"result":{"terminal":{"tail":["⏺\udf13"],"source":"screen"}}}"#
            });
        });
        let answer = dispatch(&world, "7");
        if started {
            assert_eq!(answer.ok()["started"], true);
        } else {
            let error = answer.error();
            assert!(error.contains("started no turn"), "{error}");
            assert!(error.contains("Screen's last lines:\n⏺�"), "{error}");
            assert!(!error.contains("screen unavailable"), "{error}");
            assert!(world.orca_calls().iter().any(|call| common::asked(call) == "terminal read"));
        }
        let orca = world.orca();
        let sent: usize = orca["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|terminal| terminal["sent"].as_array().unwrap().len())
            .sum();
        assert_eq!(sent, 1, "the brief is submitted exactly once");
    }
}

#[test]
fn an_orca_error_with_a_lone_surrogate_and_non_json_output_still_fail() {
    for (text, reason) in [
        (
            r#"{"ok":false,"error":{"code":"agent_prompt_blocked","details":{"reason":"Reason ⏺\udf13"}}}"#,
            "agent_prompt_blocked: Reason ⏺�",
        ),
        (
            r#"{"ok":false,"error":{"code":"terminal_not_writable","message":"⏺\ud800"}}"#,
            "orca terminal send failed: terminal_not_writable",
        ),
        ("not JSON", "orca terminal send failed: not JSON"),
    ] {
        let world = ready_world();
        world.with_orca(|orca| orca["raw_answers"] = json!({"terminal send": text}));
        let error = dispatch(&world, "7").error();
        assert!(error.contains(reason), "{error}");
    }
}

#[test]
fn a_brief_orca_cannot_observe_is_delivered_for_worker_health_to_judge() {
    let world = ready_world();
    world.with_orca(|orca| {
        orca["raw_answers"] = json!({
            "terminal wait": r#"{"ok":true,"result":{"wait":{"satisfied":true,"preview":"⏺"}}}"#,
            "terminal send": r#"{"ok":true,"result":{"send":{"accepted":true,
                "prompt":{"stages":["input_accepted"],"observation":"unsupported",
                "provider":"unsupported"}}}}"#,
            "terminal read": r#"{"ok":true,"result":{"terminal":{"tail":["⏺ Working…"],"source":"screen"}}}"#
        });
    });
    let answer = dispatch(&world, "7").ok();
    assert_eq!(answer["started"], true, "{answer}");
    let sent: usize = world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|terminal| terminal["sent"].as_array().unwrap().len())
        .sum();
    assert_eq!(sent, 1, "the brief is submitted exactly once");
}

#[test]
fn an_unobservable_accepted_brief_without_a_working_screen_is_a_failed_start() {
    let world = ready_world();
    world.with_orca(|orca| {
        orca["raw_answers"] = json!({
            "terminal wait": r#"{"ok":true,"result":{"wait":{"satisfied":true,"preview":"❯"}}}"#,
            "terminal send": r#"{"ok":true,"result":{"send":{"accepted":true,
                "prompt":{"stages":["input_accepted"],"observation":"unsupported",
                "provider":"unsupported"}}}}"#,
            "terminal read": r#"{"ok":true,"result":{"terminal":{"tail":["❯"],"source":"screen"}}}"#
        });
    });

    let error = dispatch(&world, "7").error();
    assert!(error.contains("started no turn"), "{error}");
    assert!(error.contains("\"observation\":\"unsupported\""), "{error}");
    let sent: usize = world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|terminal| terminal["sent"].as_array().unwrap().len())
        .sum();
    assert_eq!(sent, 1, "the brief is submitted exactly once");
}

#[test]
fn an_accepted_brief_that_starts_no_turn_is_a_failed_start_on_every_harness() {
    for harness in afkconvoy_core::harness::Harness::ALL {
        for screen in [
            include_str!("screens/kimi-migration.txt"),
            include_str!("screens/kimi-empty-input.txt"),
        ] {
            let world = ready_world();
            world.with_kit(|kit| {
                kit["harnesses"] = json!([{"harness":harness.key(), "command":harness.stock(),
                    "models":[{"model":"test-model"}]}]);
            });
            world.with_orca(|orca| {
                orca["prompt_stages"] = json!(["input_accepted"]);
                orca["send_screen"] = json!(screen.lines().collect::<Vec<_>>());
            });
            let error = world
                .afk(&[
                    "dispatch",
                    "--issue",
                    "7",
                    "--instance",
                    "alpha",
                    "--host",
                    "h1",
                    "--now",
                    "1000",
                ])
                .error();
            assert!(error.contains("started no turn"), "{}: {error}", harness.key());
            assert!(error.contains(screen.trim()), "screen evidence missing: {error}");
            let orca = world.orca();
            let sent: usize = orca["terminals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["sent"].as_array().unwrap().len())
                .sum();
            assert_eq!(sent, 1, "an unproven send is never resent");
        }
    }
}

#[test]
fn a_first_run_menu_that_is_not_input_is_a_failed_start_without_typing() {
    let world = ready_world();
    let screen = include_str!("screens/kimi-migration.txt");
    world.with_orca(|orca| {
        orca["never_ready"] = json!(true);
        orca["start_screen"] = json!(screen.lines().collect::<Vec<_>>());
    });
    let error = dispatch(&world, "7").error();
    assert!(error.contains("not ready") && error.contains(screen.trim()), "{error}");
    assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal send"));
}

#[test]
fn a_worker_is_started_under_nice_with_a_tmpdir_its_worktree_ignores_and_takes_with_it() {
    let world = ready_world();
    let started = dispatch(&world, "7").ok();
    let worktree = Path::new(started["worktree"].as_str().unwrap()).to_path_buf();
    let calls = world.orca_calls();
    let created = calls.iter().find(|call| common::asked(call) == "terminal create").unwrap();
    let line = format!("export AFKCONVOY_ISSUE=7 && {}", common::trusted(COMMAND));
    assert_eq!(created[4..6], ["--command".to_string(), common::niced(&world, &worktree, &line)]);

    // what the worker writes there is in its worktree, and not git's to show…
    let tmp = world.tmp(&worktree);
    std::fs::write(tmp.join("scratch"), "the worker's own").unwrap();
    std::fs::create_dir_all(tmp.join("a/b")).unwrap();
    std::fs::write(tmp.join("a/b/deep"), "and deeper").unwrap();
    assert!(worktree.join(".afkconvoy-tmp/a/b/deep").exists());
    assert_eq!(git(&worktree, &["status", "--porcelain", "--untracked-files=all"]), "");

    // …and goes when the worktree does
    git(&world.clone, &["worktree", "remove", "--force", worktree.to_str().unwrap()]);
    assert!(!tmp.exists() && !worktree.exists());
}

#[test]
fn dispatch_starts_a_worker_on_a_fresh_worktree() {
    let world = ready_world();
    let started = dispatch(&world, "7").ok();
    assert_eq!(started["started"], true);
    assert_eq!(started["claim"], "won");
    assert_eq!(started["branch"], "tester/issue-7-add-the-plan-command");
    let worktree = Path::new(started["worktree"].as_str().unwrap());
    assert!(worktree.is_dir());

    // the claim is alpha's
    let scan = world.afk(&["scan"]).ok();
    assert_eq!(scan["claims"][0]["instance"], "alpha");

    // one terminal in the worktree, started with the command verbatim
    let orca = world.orca();
    let open: Vec<&Value> =
        orca["terminals"].as_array().unwrap().iter().filter(|t| t["open"] == true).collect();
    assert_eq!(open.len(), 1, "the bare shell orca opened is closed");
    assert_eq!(open[0]["command"], common::trusted(COMMAND));
    assert_eq!(open[0]["handle"], started["terminal"]);

    // the prompt went to a brief file; the terminal got a one-line pointer, submitted
    let sent = open[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["enter"], true);
    let pointer = sent[0]["text"].as_str().unwrap();
    let brief = Path::new(git(worktree, &["rev-parse", "--absolute-git-dir"]).as_str())
        .join("convoy-worker-prompt.md");
    assert!(pointer.contains(brief.to_str().unwrap()), "{pointer}");
    let prompt = std::fs::read_to_string(&brief).unwrap();
    assert!(prompt.contains("**Your issue:** #7 — Add the `plan` command"));
    assert!(prompt.contains("**Your branch:** `tester/issue-7-add-the-plan-command`"));
    assert!(prompt.contains(&format!("**Your worktree:** `{}`", worktree.display())));
    assert!(prompt.contains(
        "The next command holds one turn through merge, gate, recording green and landing."
    ));
    assert!(prompt.contains(&format!("{} land --issue 7\n", env!("CARGO_BIN_EXE_afkconvoy"))));
    assert!(!prompt.contains("{n}") && !prompt.contains("{branch}"));
    assert_eq!(git(worktree, &["status", "--porcelain"]), "", "the brief is not in the worktree");

    // the branch orca cut is recorded on the issue
    assert_eq!(world.comments(7).len(), 1);
    assert!(
        world.comments(7)[0]
            .starts_with("<!--convoy:branch name=tester/issue-7-add-the-plan-command-->")
    );
}

#[test]
fn the_steps_run_in_order() {
    let world = ready_world();
    world.watch_pushes(None);
    dispatch(&world, "7").ok();
    assert_eq!(
        world.pushes(),
        ["refs/convoy/target/7", "refs/convoy/heartbeat/alpha", "refs/convoy/claim/7"]
    );
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert_eq!(
        calls,
        [
            // what survived is read first, under the claim
            "repo list",
            "worktree list",
            "repo list",
            "worktree create",
            "terminal list",
            "terminal close",
            // A close is verified before a replacement can write here.
            "terminal list",
            "terminal create",
            "terminal wait",
            "terminal wait",
            // Screen quiet alone does not end a harness's startup episode.
            "worktree ps",
            "terminal send",
            // The successful delivery records Orca's harness for the host watch.
            "worktree ps"
        ]
    );
    let delivered =
        std::fs::read_to_string(world.cache().join("afkconvoy/watch/delivered.json")).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&delivered).unwrap(), json!(["claude-code"]));
}

#[test]
fn a_lost_claim_race_starts_nothing() {
    let world = ready_world();
    world.afk(&["claim", "--issue", "7", "--instance", "beta", "--now", "1000"]).ok();
    let lost = dispatch(&world, "7").ok();
    assert_eq!(lost["started"], false);
    assert_eq!(lost["claim"], "lost");
    assert_eq!(lost["owner"]["instance"], "beta");
    assert_eq!(world.orca_calls(), [] as [Vec<String>; 0]);
    assert_eq!(world.comments(7), [] as [String; 0]);
}

#[test]
fn a_claim_already_held_is_not_pushed_again() {
    let world = ready_world();
    world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "1000"]).ok();
    world.watch_pushes(None);
    let started = dispatch(&world, "7").ok();
    assert_eq!(started["claim"], "held");
    assert_eq!(world.pushes(), [] as [String; 0]);
}

#[test]
fn the_worktree_is_cut_from_what_the_remote_has() {
    let world = ready_world();
    // the fleet's local base branch is behind the remote
    let landed = world.land_elsewhere("b.txt");
    assert_ne!(git(&world.clone, &["rev-parse", BASE]), landed);

    let started = dispatch(&world, "7").ok();
    let worktree = Path::new(started["worktree"].as_str().unwrap());
    assert_eq!(git(worktree, &["rev-parse", "HEAD"]), landed);
    let calls = world.orca_calls();
    let create = calls.iter().find(|call| common::asked(call) == "worktree create").unwrap();
    let at = create.iter().position(|arg| arg == "--base-branch").unwrap();
    assert_eq!(create[at + 1], landed, "handed over as a sha, not a branch name");
}

#[test]
fn a_worktree_orca_cut_behind_the_tip_is_brought_up_to_it() {
    let world = ready_world();
    let behind = git(&world.clone, &["rev-parse", "HEAD"]);
    let landed = world.land_elsewhere("b.txt");
    world.with_orca(|orca| orca["stale_base"] = json!(behind));
    let started = dispatch(&world, "7").ok();
    let worktree = Path::new(started["worktree"].as_str().unwrap());
    assert_eq!(git(worktree, &["rev-parse", "HEAD"]), landed);
}

#[test]
fn a_failure_of_orca_is_an_error_with_the_claim_still_held() {
    for knob in ["create_fails", "never_ready", "send_refused", "stopped"] {
        let world = ready_world();
        world.with_orca(|orca| {
            orca[knob] = if knob == "create_fails" { json!([7]) } else { json!(true) }
        });
        let error = dispatch(&world, "7").error();
        assert!(
            error.contains("orca")
                || error.contains("not ready")
                || error.contains("did not accept"),
            "{knob}: {error}"
        );
        let scan = world.afk(&["scan"]).ok();
        assert_eq!(scan["claims"][0]["instance"], "alpha", "{knob}");
    }
}

#[test]
fn a_trust_prompt_is_a_named_trust_gap_and_the_worker_is_told_nothing() {
    let world = ready_world();
    world.set_gate_config("gate_command: ./gate.sh\nescalate_to: human");
    world.with_orca(|orca| {
        orca["trust_prompt"] = json!(true);
        orca["start_screen"] = json!(["Accessing workspace:", "Yes, I trust this folder"]);
    });
    let error = dispatch(&world, "7").error();
    assert!(error.contains("Claude Code worktree trust gap"), "{error}");
    assert!(error.contains("harness `claude-code`"), "{error}");
    assert!(error.contains(COMMAND), "{error}");
    let orca = world.orca();
    let path = orca["terminals"][0]["worktreePath"].as_str().unwrap();
    assert!(error.contains(path), "{error}");
    let terminal = orca["terminals"].as_array().unwrap().iter().find(|t| t["open"] == true);
    assert!(terminal.is_none(), "the trust question is never answered at the terminal");
    assert!(orca["terminals"].as_array().unwrap().iter().all(|t| t["sent"] == json!([])));
    let comments = world.comments(7);
    let escalations: Vec<_> =
        comments.iter().filter(|body| body.contains("convoy:escalation")).collect();
    assert_eq!(escalations.len(), 1, "{comments:?}");
    assert!(escalations[0].contains("worktree trust gap"), "{comments:?}");
    let evidence = escalations[0];
    for raw in ["agent-trust-workspace", "satisfied", "Yes, I trust this folder", "source"] {
        assert!(evidence.contains(raw), "startup evidence lost: {evidence}");
    }
    assert!(evidence.contains(orca["terminals"][0]["line"].as_str().unwrap()), "{evidence}");
    let calls = world.orca_calls();
    let read = calls.iter().position(|call| common::asked(call) == "terminal read").unwrap();
    let close = calls
        .iter()
        .rposition(|call| matches!(common::asked(call).as_str(), "terminal close" | "worktree rm"))
        .unwrap();
    assert!(read < close, "capture precedes terminal cleanup: {calls:?}");
}

#[test]
fn a_trust_gap_keeps_the_screen_read_failure_and_the_wait_that_classified_it() {
    let world = ready_world();
    world.set_gate_config("gate_command: ./gate.sh\nescalate_to: human");
    world.with_orca(|orca| {
        orca["trust_prompt"] = json!(true);
        orca["read_fails"] = json!(true);
    });
    dispatch(&world, "7").error();
    let comments = world.comments(7).join("\n");
    assert!(comments.contains("agent-trust-workspace"), "{comments}");
    assert!(comments.contains("terminal_not_readable"), "{comments}");
    assert!(world.orca()["terminals"].as_array().unwrap().iter().all(|t| t["open"] == false));
}

#[test]
fn a_trust_gap_keeps_screen_unavailability_and_redacts_echoed_credentials() {
    let world = ready_world();
    world.set_gate_config("gate_command: ./gate.sh\nescalate_to: human");
    world.kit_command("claude-code", COMMAND);
    let secret = "startup-\"quoted'-secret-103";
    world.with_orca(|orca| {
        orca["trust_prompt"] = json!(true);
        orca["screen_source"] = json!("screen-unavailable");
        orca["start_screen"] = json!([format!("provider credential: {secret}")]);
    });
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("ANTHROPIC_AUTH_TOKEN", secret)
        .args(["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let comments = world.comments(7).join("\n");
    assert!(comments.contains("screen-unavailable"), "{comments}");
    assert!(comments.contains("provider credential: [redacted]"), "{comments}");
    assert!(!comments.contains(secret), "{comments}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(secret));
}

#[test]
fn an_orca_that_does_not_know_the_repo_is_an_error() {
    let world = ready_world();
    world.with_orca(|orca| orca["repos"] = json!([]));
    assert!(dispatch(&world, "7").error().contains("orca knows no repo"));
}

#[test]
fn a_closed_issue_is_never_claimed() {
    let world = ready_world();
    world.with_gh(|gh| gh["issues"][0]["state"] = json!("closed"));
    assert!(dispatch(&world, "7").error().contains("closed"));
    assert_eq!(world.afk(&["scan"]).ok()["claims"], json!([]));
}

#[test]
fn a_base_branch_the_remote_lacks_is_an_error() {
    let world = ready_world();
    let config = world.clone.join("docs/agents/afkconvoy.md");
    std::fs::write(config, "```yaml\nbase_branch: release\ngate_command: make\n```\n").unwrap();
    assert!(dispatch(&world, "7").error().contains("the remote has no branch \"release\""));
}

#[test]
fn the_branch_is_recorded_once_however_often_it_is_asked() {
    let world = ready_world();
    world.with_gh(|gh| {
        gh["comments"]["7"] = json!([{"id": 1, "html_url": "u",
            "body": "<!--convoy:branch name=tester/issue-7-add-the-plan-command-->\nearlier"}]);
    });
    dispatch(&world, "7").ok();
    assert_eq!(world.comments(7).len(), 1);
}

#[test]
fn an_open_blocker_read_before_the_claim_claims_and_starts_nothing() {
    let world = ready_world();
    world.issue(71, &["bug"]);
    world.issue(72, &["bug"]);
    world.with_gh(|gh| gh["edges"]["7"] = json!([72, 71]));
    let skipped = dispatch(&world, "7").ok();
    assert_eq!(
        skipped,
        json!({"issue": 7, "started": false, "skipped": "blocked", "blocked_by": [71, 72],
            "woke": false})
    );
    assert_eq!(world.claims(), [] as [(u64, String); 0]);
    assert_eq!(world.orca_calls(), [] as [Vec<String>; 0]);
    assert_eq!(world.comments(7), [] as [String; 0]);

    // a blocker that closed holds nothing back
    assert!(
        world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/target/7"]).is_empty()
    );
    world.close(71, "completed");
    world.close(72, "completed");
    assert_eq!(dispatch(&world, "7").ok()["started"], true);
}

#[test]
fn a_blocker_added_after_the_rebuild_gives_the_slot_to_the_next_frontier_issue() {
    let world = World::new();
    for n in [68, 69, 70, 71] {
        world.issue(n, &["ready-for-agent"]);
    }
    // the rebuild reads no blocker of #68–#70: GitHub records them as it answers the dispatch
    world.with_gh(|gh| {
        for n in ["68", "69", "70"] {
            gh["edges"][n] = json!([71]);
        }
        for row in gh["issues"].as_array_mut().unwrap() {
            row["blockers_at_rebuild"] = json!([]);
        }
    });
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    let frontier: Vec<&Value> = plan["frontier"].as_array().unwrap().iter().collect();
    assert_eq!(frontier.len(), 4, "{plan}");
    let read: Vec<Vec<String>> = world.gh_calls();
    let dependencies =
        read.iter().filter(|call| call.iter().any(|arg| arg.contains("dependencies")));
    assert_eq!(dependencies.count(), 0, "plan reads nothing more: {read:?}");

    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    let ticked = {
        world.kit_command("claude-code", COMMAND);
        world.afk(&args)
    }
    .ok();
    let did = ticked["did"].as_array().unwrap();
    let dispatches: Vec<&Value> = did.iter().filter(|step| step["do"] == "dispatch").collect();
    assert_eq!(
        dispatches,
        [
            &json!({"do": "dispatch", "issue": 68, "outcome": "blocked", "blocked_by": [71]}),
            &json!({"do": "dispatch", "issue": 69, "outcome": "blocked", "blocked_by": [71]}),
            &json!({"do": "dispatch", "issue": 70, "outcome": "blocked", "blocked_by": [71]}),
            &json!({"do": "dispatch", "issue": 71, "outcome": "done"}),
        ]
    );
    assert_eq!(world.claims(), [(71, "alpha".to_string())]);
    assert_eq!((&ticked["held"], &ticked["frontier_remaining"]), (&json!(1), &json!(0)));
    for n in [68, 69, 70] {
        assert_eq!(world.comments(n), [] as [String; 0], "no branch, no status board on #{n}");
    }
    assert_eq!(world.boards(71).len(), 1);
    assert_eq!(world.open_terminals().len(), 1);
}

#[test]
fn a_stale_dependency_summary_does_not_hold_a_closed_blockers_issue_out_of_the_frontier() {
    let world = ready_world();
    world.issue(71, &["bug"]);
    world.close(71, "completed");
    world.with_gh(|gh| {
        gh["edges"]["7"] = json!([71]);
        gh["issues"][0]["blocked_by"] = json!(1);
    });
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 7, "{plan}");
    let ticked = world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
    assert_eq!(world.claims(), [(7, "alpha".to_string())], "{ticked}");
}

#[test]
fn a_fork_s_pr_that_closes_a_ready_issue_holds_it_off_from_nobody() {
    let world = ready_world();
    world.fork_pr(98, 7, "patch-1");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 7, "{plan}");
    let ticked = world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
    assert_eq!(world.claims(), [(7, "alpha".to_string())], "{ticked}");

    // Its worker starts the issue afresh: the fork's PR is no work of its own.
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == 7).expect("a worktree of issue 7");
    let worktree = Path::new(row["path"].as_str().unwrap());
    let brief = Path::new(git(worktree, &["rev-parse", "--absolute-git-dir"]).as_str())
        .join("convoy-worker-prompt.md");
    let prompt = std::fs::read_to_string(&brief).unwrap();
    assert!(prompt.contains("**Implement**") && !prompt.contains("#98"), "{prompt}");
    assert_eq!(world.comments(98), [] as [String; 0]);
}

mod common;

use std::path::Path;

use common::{BASE, World, git};
use serde_json::{Value, json};

const COMMAND: &str = "ckimi --flag 'quoted arg'";

fn dispatch(world: &World, issue: &str) -> common::Answer {
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
        "--worker-command",
        COMMAND,
    ])
}

fn ready_world() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.with_gh(|gh| gh["issues"][0]["title"] = json!("Add the `plan` command"));
    world
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
    assert_eq!(open[0]["command"], COMMAND);
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
    assert!(prompt.contains(&format!("{} gate --train\n", env!("CARGO_BIN_EXE_afkconvoy"))));
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
    assert_eq!(world.pushes(), ["refs/convoy/heartbeat/alpha", "refs/convoy/claim/7"]);
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert_eq!(
        calls,
        [
            // what survived is read first, under the claim
            "repo list",
            "worktree list",
            "repo list",
            "worktree create",
            "terminal close",
            "terminal create",
            "terminal wait",
            "terminal send"
        ]
    );
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
fn a_worker_asking_whether_its_folder_is_trusted_is_answered_yes_then_briefed() {
    let world = ready_world();
    world.with_orca(|orca| orca["trust_prompt"] = json!(true));
    dispatch(&world, "7").ok();
    let orca = world.orca();
    let terminal = orca["terminals"].as_array().unwrap().iter().find(|t| t["open"] == true);
    let terminal = terminal.expect("the worker's terminal");
    assert_eq!(terminal["trusted"], true);
    // the brief went to the agent, not to the question
    let sent = terminal["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0]["text"].as_str().unwrap().contains("convoy-worker-prompt.md"));
}

#[test]
fn a_worker_that_asks_again_once_answered_is_an_error_and_is_told_nothing() {
    let world = ready_world();
    world.with_orca(|orca| {
        orca["trust_prompt"] = json!(true);
        orca["trust_sticks"] = json!(true);
    });
    let error = dispatch(&world, "7").error();
    assert!(error.contains("still asks whether to trust the folder"), "{error}");
    let orca = world.orca();
    let terminal = orca["terminals"].as_array().unwrap().iter().find(|t| t["open"] == true);
    let terminal = terminal.expect("the worker's terminal");
    assert_eq!(terminal["sent"], json!([]));
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
            row["blocked_by"] = json!(0);
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
    let ticked = world.afk(&[&args[..], &["--worker-command", COMMAND]].concat()).ok();
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

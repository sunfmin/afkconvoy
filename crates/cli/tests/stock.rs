//! The stock consult of an unbound repo (docs/adr/0018): an escalated issue
//! goes to it before a human, and each of its answers is read back.

use crate::common;

use std::path::PathBuf;

use common::World;
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
        .filter(|step| !["board", "heartbeat"].contains(&step["do"].as_str().unwrap()))
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// The latest worker's terminal reports `state` since `since`, its last
/// output then too.
fn report(world: &World, state: &str, since: u64) {
    world.with_orca(|orca| {
        let terminal = orca["terminals"].as_array_mut().unwrap().last_mut().unwrap();
        terminal["agent"] = json!({"state": state, "stateStartedAt": since * 1000});
        terminal["lastOutputAt"] = json!(since * 1000);
    });
}

/// The latest worktree cut for issue 7.
fn worktree(world: &World) -> PathBuf {
    let worktrees = world.orca()["worktrees"].clone();
    PathBuf::from(worktrees.as_array().unwrap().last().unwrap()["path"].as_str().unwrap())
}

/// The brief of the latest worker started on issue 7.
fn brief(world: &World) -> String {
    let dir = worktree(world);
    let git_dir = common::git(&dir, &["rev-parse", "--absolute-git-dir"]);
    std::fs::read_to_string(PathBuf::from(git_dir).join("convoy-worker-prompt.md")).unwrap()
}

/// Sets the labels issue `number` carries, as a `gh issue edit` would.
fn relabel(world: &World, number: u64, labels: &[&str]) {
    world.with_gh(|gh| {
        for issue in gh["issues"].as_array_mut().unwrap() {
            if issue["number"] == number {
                issue["labels"] = json!(labels);
            }
        }
    });
}

/// A world whose worker on issue 7 let it through to a human, saying `why`;
/// `yaml` is the rest of its config.
fn escalated(yaml: &str) -> World {
    let world = World::new();
    world.set_gate_config(&format!("gate_command: ./gate.sh\ngate_seconds: 600\n{yaml}"));
    world.issue(7, &["ready-for-agent", "bug"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    world.comment(7, "<!--convoy:verdict phase=escalate-->\nThe schema is ambiguous.");
    assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    world
}

/// The same, with its stock consult started on it at 1180.
fn asked() -> World {
    let world = escalated("");
    assert_eq!(did(&tick(&world, 1180)), ["dispatch 7 done"]);
    world
}

#[test]
fn an_escalated_issue_goes_to_the_stock_consult_first() {
    let world = asked();
    let claims = world.claims();
    assert_eq!(claims.len(), 1, "the consult holds a claim of its own");
    let said = world.comments(7);
    let marker = said.last().unwrap();
    assert!(marker.starts_with("<!--convoy:consult claim="), "{said:?}");
    // It is the human's labels that the issue carries while the consult looks.
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    let brief = brief(&world);
    assert!(brief.contains("started as the **stock consult**"), "{brief}");
    assert!(
        brief.contains("# The stock consult\n\nYou stand where a human would stand"),
        "{brief}"
    );
    assert!(
        brief.contains("> The worker declared `escalate`: https://gh/c/"),
        "what it is asked: {brief}"
    );
    assert!(
        brief
            .contains("gh issue edit 7 --add-label ready-for-agent --remove-label ready-for-human"),
        "{brief}"
    );
    // Not a human's while it looks.
    let plan = world.afk(&["plan", "--now", "1200"]).ok();
    assert_eq!(plan["waiting_on_human"], json!([]), "{plan}");
    // What its brief gives it to run, and any command that only reads, is
    // run in the worktree it starts in.
    assert_eq!(common::refusing_a_workers_worktree(&brief), [] as [String; 0]);
    let dir = worktree(&world);
    assert_eq!(
        world.afk_in(&dir, &["scan"]).ok()["claims"],
        json!(world.afk(&["scan"]).ok()["claims"])
    );
    world.afk_in(&dir, &["config"]).ok();
    world.afk_in(&dir, &["takeover", "--list", "--now", "1200"]).ok();
}

#[test]
fn a_consult_that_hands_the_issue_back_frees_it_for_the_next_worker() {
    let world = asked();
    world.comment(7, "Use the v2 schema: the v1 one is retired.");
    relabel(&world, 7, &["bug", "ready-for-agent"]);
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["judge 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["bug", "ready-for-agent"], "as the consult left them");
    // The next worker works the issue, under a claim of its own.
    assert_eq!(did(&tick(&world, 1590)), ["dispatch 7 done"]);
    let brief = brief(&world);
    assert!(!brief.contains("stock consult"), "{brief}");
    // …and what it hands to a human next is a human's: the consult answered the last.
    world.comment(7, "<!--convoy:verdict phase=escalate-->\nStill ambiguous.");
    assert_eq!(did(&tick(&world, 1680)), ["judge 7 done"]);
    assert_eq!(did(&tick(&world, 1770)), [] as [&str; 0]);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
}

#[test]
fn a_consult_that_closes_the_issue_ends_its_claim() {
    let world = asked();
    world.close(7, "completed");
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["settle 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(world.state(7), "closed");
}

#[test]
fn a_consult_that_opens_an_issue_it_waits_on_hands_it_back_blocked() {
    let world = asked();
    world.issue(8, &["ready-for-agent"]);
    world.with_gh(|gh| gh["edges"]["7"] = json!([8]));
    relabel(&world, 7, &["bug", "ready-for-agent"]);
    report(&world, "done", 1200);
    let ticked = tick(&world, 1500);
    assert!(did(&ticked).contains(&"judge 7 done".to_string()), "{ticked}");
    assert_eq!(world.claims().iter().filter(|claim| claim.0 == 7).count(), 0);
    let plan = world.afk(&["plan", "--now", "1510"]).ok();
    let excluded = plan["excluded"].as_array().unwrap();
    let seven = excluded.iter().find(|row| row["number"] == 7).expect("7 is held back");
    assert_eq!(seven["reason"], "open blockers: #8", "{plan}");
}

#[test]
fn a_consult_that_lets_the_issue_through_leaves_it_with_a_human() {
    let world = asked();
    let said = world.comment(7, "<!--convoy:verdict phase=escalate-->\nOnly the owner can say.");
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["judge 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    let last = world.comments(7).last().unwrap().clone();
    assert!(last.starts_with("<!--convoy:escalation claim="), "{last}");
    assert!(
        last.contains("The stock consult was asked of this issue and let it through"),
        "{last}"
    );
    assert!(last.contains(&said), "{last}");
    // No consult is asked again: it is a human's.
    assert_eq!(did(&tick(&world, 1590)), [] as [&str; 0]);
    let plan = world.afk(&["plan", "--now", "1600"]).ok();
    assert_eq!(plan["waiting_on_human"][0]["issue"], 7, "{plan}");
}

#[test]
fn an_unclassified_silent_stock_consult_keeps_its_claim_for_diagnosis() {
    let world = asked();
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["judge 7 nothing"]);
    assert_eq!(did(&tick(&world, 1800)), ["judge 7 nothing"]);
    assert_eq!(world.claims().len(), 1);
    assert!(!world.comments(7).join("\n").contains("went silent"));
}

#[test]
fn escalate_to_human_sends_every_hand_off_to_a_human_as_before() {
    let world = escalated("escalate_to: human");
    assert_eq!(did(&tick(&world, 1180)), [] as [&str; 0]);
    assert_eq!(world.claims(), []);
    assert!(!world.comments(7).iter().any(|body| body.contains("convoy:consult")));
    let plan = world.afk(&["plan", "--now", "1200"]).ok();
    assert_eq!(plan["waiting_on_human"][0]["issue"], 7, "{plan}");
}

#[test]
fn a_stock_consult_that_leaves_a_commit_has_failed_and_goes_to_a_human() {
    let world = asked();
    common::commit(&worktree(&world), "schema.txt", "v1\n");
    // …and hands the issue back, as though that were its answer.
    relabel(&world, 7, &["bug", "ready-for-agent"]);
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["judge 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    let last = world.comments(7).last().unwrap().clone();
    assert!(
        last.contains(
            "A consult writes no code, and the stock consult left a commit on the branch"
        ),
        "{last}"
    );
    assert_eq!(did(&tick(&world, 1590)), [] as [&str; 0]);
}

#[test]
fn an_unbound_stock_consult_parks_with_a_reason_until_the_ready_label_is_restored() {
    let world = asked();
    world
        .afk(&[
            "verdict",
            "park",
            "--consult",
            "--issue",
            "7",
            "--stage",
            "consult",
            "--text",
            "Wait for raw failure output.",
        ])
        .ok();
    assert!(world.comments(7).last().unwrap().contains("Wait for raw failure output."));
    assert_eq!(world.labels(7), ["bug", "convoy:parked"]);
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["judge 7 done"]);
    assert_eq!(world.claims(), []);
    let parked = world.comments(7).pop().unwrap();
    let url = world.comment_url(7, &parked);
    world.comment(7, "Routine status comment.");
    for command in ["plan", "doctor"] {
        let view = world.afk(&[command, "--now", "1550"]).ok();
        assert_eq!(view["waiting_on_human"][0]["issue"], 7, "{view}");
        assert_eq!(
            view["waiting_on_human"][0]["why"],
            "parked for evidence: Wait for raw failure output."
        );
        assert_eq!(view["waiting_on_human"][0]["url"], url);
    }
    world.comment(7, "Routine status comment.");
    assert_eq!(did(&tick(&world, 1590)), [] as [&str; 0]);
    world.comment(7, "New raw failure output.");
    relabel(&world, 7, &["bug", "ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1680)), ["dispatch 7 done"]);
}

#[test]
fn only_the_claimed_stock_consult_can_declare_an_unbound_close() {
    let world = asked();
    let args = [
        "verdict",
        "close",
        "--issue",
        "7",
        "--stage",
        "consult",
        "--text",
        "Nothing remains to investigate.",
    ];
    let before = world.comments(7).len();
    assert!(world.afk(&args).error().contains("only the stock consult"));
    assert_eq!(world.comments(7).len(), before);
    world.afk(&[&args[..], &["--consult"]].concat()).ok();
    assert!(world.comments(7).last().unwrap().contains("Nothing remains to investigate."));
    report(&world, "done", 1200);
    assert_eq!(did(&tick(&world, 1500)), ["settle 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1590)), [] as [&str; 0]);
}

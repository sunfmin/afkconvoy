//! The fleet lock (docs/adr/0011): an open issue carrying the lock label stops
//! the fleet from working anything else, and closing the last one ends it.

mod common;

use common::{World, write};
use serde_json::{Value, json};

const AGENT: [&str; 2] = ["--worker-command", "agent"];

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&[&args[..], &AGENT].concat()).ok()
}

/// What a tick did, but for the status boards it brought up to date.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    steps
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn numbers(rows: &Value) -> Vec<u64> {
    rows.as_array().unwrap().iter().map(|row| row["number"].as_u64().unwrap()).collect()
}

fn reason(plan: &Value, issue: u64) -> String {
    let rows = plan["excluded"].as_array().unwrap();
    let row = rows.iter().find(|row| row["number"] == issue).expect("an excluded issue");
    row["reason"].as_str().unwrap().to_string()
}

fn lock(world: &World, issue: u64, more: &[&str]) -> common::Answer {
    let issue = issue.to_string();
    world.afk(&[&["lock", "--issue", &issue][..], more].concat())
}

fn land(world: &World, dir: &std::path::Path, issue: u64) -> Value {
    world.afk_in(dir, &["land", "--issue", &issue.to_string(), "--now", "7000"]).ok()
}

#[test]
fn with_a_lock_standing_the_frontier_is_the_lock_issues_and_plan_says_which() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    // In an unbound repo a lock issue is worked when it also carries the
    // ready label: this one stands, and nobody is started on it.
    world.issue(8, &["fleet-lock"]);
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["locks"], json!([7, 8]));
    assert_eq!(numbers(&plan["frontier"]), [7]);
    assert_eq!(reason(&plan, 3), "a fleet lock stands (#7, #8)");
    assert_eq!(reason(&plan, 8), "no ready-for-agent label");

    // No lock stands: `plan` says so, and the frontier is what it was.
    world.close_issue(7);
    world.close_issue(8);
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["locks"], json!([]));
    assert_eq!(numbers(&plan["frontier"]), [3]);
}

#[test]
fn running_workers_are_left_alone_and_what_they_declared_is_acted_on_once_the_lock_ends() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(5, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent"]);
    let first = tick(&world, 1000);
    assert_eq!(did(&first), ["dispatch 3 done", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(first["locks"], json!([]));

    // The base branch broke. #7 carries the fix, and #3's worker — silent
    // ever since — declared that its issue needs nothing.
    world.issue(7, &["ready-for-agent"]);
    assert_eq!(lock(&world, 7, &[]).ok()["raised"], true);
    world.comment(3, "<!--convoy:verdict phase=already-satisfied-->\nnothing to do");
    world.gh_calls();
    let locked = tick(&world, 1390);
    assert_eq!(
        did(&locked),
        ["dispatch 7 done", "heartbeat done"],
        "the fleet was at its bound: the lock issue does not wait for a slot, and nobody \
         asks after the workers of #3 and #5"
    );
    assert_eq!(locked["locks"], json!([7]));
    assert_eq!((&locked["held"], &locked["frontier_remaining"]), (&json!(3), &json!(0)));
    assert_eq!(world.state(3), "open", "its verdict waits for the lock to end");
    let typed = world.orca_calls().iter().any(|call| common::asked(call) == "terminal send");
    assert!(typed, "the lock issue's worker was handed its brief");

    // Still locked: only the lock issue's worker is asked after (silent, with
    // no clock to read, it is nudged), and #9 stays where it is.
    assert_eq!(did(&tick(&world, 1480)), ["judge 7 nudged", "heartbeat done"]);

    // The fix landed: #7 is closed, and the next tick resumes everything.
    world.close_issue(7);
    let resumed = tick(&world, 1570);
    assert_eq!(resumed["locks"], json!([]));
    let did = did(&resumed);
    assert_eq!(did[..2], ["settle 7 done", "judge 3 done"], "{did:?}");
    assert!(did.contains(&"dispatch 9 done".to_string()), "{did:?}");
    assert_eq!(world.state(3), "closed", "what #3's worker declared was acted on");
}

#[test]
fn the_line_of_a_tick_says_that_a_lock_stands_and_which_issue_it_is() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    let args = ["run", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    let paced = ["--ticks", "1", "--second-ms", "0"];
    let (lines, _) = world.afk_lines(&[&args[..], &AGENT, &paced].concat());
    let line = lines.iter().find(|line| line.starts_with("tick 1: ")).expect("a tick's line");
    assert!(
        line.starts_with("tick 1: FLEET LOCK #7 stands: only it is worked · dispatched #7 · "),
        "{line}"
    );
    assert_eq!(world.claims().iter().map(|claim| claim.0).collect::<Vec<_>>(), [7]);
}

#[test]
fn with_a_lock_standing_no_pr_of_another_issue_joins_the_landing_train() {
    let world = World::new();
    world.set_gate("true");
    let seven = world.worker(7, "seven.txt", "7\n");
    let eight = world.worker(8, "eight.txt", "8\n");
    lock(&world, 8, &[]).ok();
    world.watch_pushes(None);

    let refused = land(&world, &seven, 7);
    assert_eq!(refused["outcome"], "locked");
    assert_eq!(refused["locks"], json!([8]));
    assert!(refused["detail"].as_str().unwrap().contains("a fleet lock stands (#8)"));
    assert_eq!(world.pushes(), [] as [String; 0], "nothing joined, and nothing was pushed");

    // The lock issue's own PR joins: it carries the fix.
    assert_eq!(land(&world, &eight, 8)["outcome"], "joined");
    // …and once it has closed, so does every other.
    world.close_issue(8);
    assert_eq!(land(&world, &seven, 7)["outcome"], "joined");
}

#[test]
fn lock_raises_a_lock_once_and_leaves_a_record_of_the_role_that_did() {
    let world = World::new();
    world.issue(7, &["bug"]);
    world.issue(9, &["fleet-lock"]);

    let raised = lock(&world, 7, &["--role", "watch"]).ok();
    assert_eq!(
        raised,
        json!({"issue": 7, "label": "fleet-lock", "role": "watch", "raised": true,
            "locks": [7, 9], "woke": false})
    );
    assert_eq!(world.labels(7), ["bug", "fleet-lock"]);
    let comments = world.comments(7);
    assert_eq!(comments.len(), 1);
    assert!(comments[0].starts_with("<!--convoy:lock role=watch-->\n"), "{}", comments[0]);
    assert!(comments[0].contains("a worker in the role `watch` raised a fleet lock"));

    // A lock that stands is not raised again, whoever asks.
    let again = lock(&world, 7, &["--role", "dev"]).ok();
    assert_eq!((&again["raised"], &again["role"]), (&json!(false), &json!("dev")));
    assert_eq!(world.comments(7).len(), 1);
    let human = lock(&world, 9, &[]).ok();
    assert_eq!(human["raised"], false, "a human put the label on it: there is nothing to add");
    assert_eq!(world.comments(9).len(), 0);

    // A worker started in its role is on record without saying so.
    world.issue(11, &[]);
    let args = ["lock", "--issue", "11"];
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args)
        .env("AFKCONVOY_ROLE", "patrolman")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(world.comments(11)[0].starts_with("<!--convoy:lock role=patrolman-->\n"));

    // A closed issue is no lock.
    world.issue(12, &[]);
    world.close_issue(12);
    assert!(lock(&world, 12, &[]).error().contains("issue #12 is closed"));
    assert_eq!(world.labels(12), [] as [String; 0]);
}

#[test]
fn an_unbound_repo_names_its_own_lock_label_and_its_worker_is_on_record_with_no_role() {
    let world = World::new();
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nlock_label: halt\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    world.issue(3, &["ready-for-agent"]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    let before = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(before["locks"], json!([]), "fleet-lock is not this repo's lock label");

    let raised = lock(&world, 3, &[]).ok();
    assert_eq!((&raised["label"], &raised["role"]), (&json!("halt"), &Value::Null));
    assert!(world.comments(3)[0].starts_with("<!--convoy:lock-->\n"));
    assert!(world.comments(3)[0].contains("a worker in no role raised a fleet lock"));
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["locks"], json!([3]));
    assert_eq!(numbers(&plan["frontier"]), [3]);
}

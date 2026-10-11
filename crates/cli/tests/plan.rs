mod common;

use common::{World, git};
use serde_json::{Value, json};

fn numbers(rows: &Value) -> Vec<u64> {
    rows.as_array().unwrap().iter().map(|row| row["number"].as_u64().unwrap()).collect()
}

fn claim(world: &World, issue: u64, instance: &str, now: u64) {
    let (issue, now) = (issue.to_string(), now.to_string());
    world.afk(&["claim", "--issue", &issue, "--instance", instance, "--now", &now]).ok();
}

/// A world with a backlog that exercises every row of a working set.
fn backlog() -> World {
    let world = World::new();
    const READY: &[&str] = &["ready-for-agent"];
    for n in [1, 2, 3, 4, 5, 6] {
        world.issue(n, READY);
    }
    world.issue(7, &["bug"]);
    world.issue(8, &["ready-for-agent", "prd"]);
    world.issue(9, READY);
    world.issue(10, READY);
    world.with_gh(|gh| {
        gh["issues"][8]["blocked_by"] = json!(1); // #9
        gh["issues"][5]["state"] = json!("closed"); // #6
        gh["issues"][9]["state"] = json!("closed"); // #10
    });
    world.pr(50, 5, "feature-5");
    world.pr(40, 4, "feature-4");
    claim(&world, 1, "me", 10_000); // mine, no PR
    claim(&world, 4, "me", 10_000); // mine, PR open
    claim(&world, 6, "me", 10_000); // mine, issue closed
    claim(&world, 2, "live", 10_000);
    claim(&world, 3, "dead", 1_000);
    claim(&world, 10, "dead", 1_000); // a dead peer's lock on a closed issue
    world
}

#[test]
fn plan_assembles_the_working_set() {
    let world = backlog();
    let plan = world.afk(&["plan", "--instance", "me", "--now", "10000"]).ok();
    assert_eq!(numbers(&plan["frontier"]), [] as [u64; 0], "{plan}");
    let excluded: Vec<(u64, &str)> = plan["excluded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["number"].as_u64().unwrap(), e["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        excluded,
        [
            (1, "already claimed"),
            (2, "already claimed"),
            (3, "already claimed"),
            (4, "already claimed"),
            (5, "has an open linked PR"),
            (7, "no ready-for-agent label"),
            (8, "epic label (prd)"),
            (9, "1 open blocker(s)"),
        ]
    );
    assert_eq!(
        plan["mine"],
        json!([
            {"number": 1, "title": "issue 1", "status": "no_pr", "pr": null},
            {"number": 4, "title": "issue 4", "status": "joining", "pr": 40},
            {"number": 6, "title": null, "status": "closed", "pr": null},
        ])
    );
    assert_eq!(numbers(&plan["peer_live"]), [2]);
    assert_eq!(numbers(&plan["stale"]), [3]);
    assert_eq!(plan["stale"][0]["instance"], "dead");
    assert_eq!(plan["stale"][0]["sha"], world.remote(&["rev-parse", "refs/convoy/claim/3"]));
    assert_eq!(numbers(&plan["stale_closed"]), [10]);
    assert_eq!(plan["free_slots"], 0, "three held under a concurrency of two");
}

#[test]
fn an_unclaimed_ready_issue_is_on_the_frontier_in_number_order() {
    let world = World::new();
    for n in [12, 3, 7] {
        world.issue(n, &["ready-for-agent"]);
    }
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(numbers(&plan["frontier"]), [3, 7, 12]);
    assert_eq!(plan["frontier"][0]["title"], "issue 3");
    assert_eq!(plan["free_slots"], 2);
}

#[test]
fn without_an_instance_every_claim_is_a_peers() {
    let world = backlog();
    let plan = world.afk(&["plan", "--now", "10000"]).ok();
    assert_eq!(plan["mine"], json!([]));
    assert_eq!(numbers(&plan["peer_live"]), [1, 2, 4, 6]);
}

#[test]
fn plan_writes_nothing() {
    let world = backlog();
    let refs_before = world.remote(&["for-each-ref"]);
    let local_before = git(&world.clone, &["for-each-ref"]);
    let gh_before = world.gh();
    world.gh_calls();
    world.watch_pushes(None);

    world.afk(&["plan", "--instance", "me", "--now", "10000"]).ok();

    assert_eq!(world.pushes(), [] as [String; 0]);
    assert_eq!(world.remote(&["for-each-ref"]), refs_before);
    assert_eq!(git(&world.clone, &["for-each-ref"]), local_before);
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "");
    let calls = world.gh_calls();
    assert!(!calls.is_empty());
    for call in &calls {
        let line = call.join(" ");
        assert!(line.starts_with("api ") || line.starts_with("pr list"), "{line}");
        let writes = ["--method", "-X", "-f", "-F"];
        assert!(!call.iter().any(|arg| writes.contains(&arg.as_str())), "{line}");
    }
    let mut gh_after = world.gh();
    gh_after["calls"] = gh_before["calls"].clone();
    assert_eq!(gh_after, gh_before);
}

#[test]
fn a_github_that_cannot_be_read_is_an_error() {
    let world = backlog();
    world.with_gh(|gh| gh["fail"] = json!(["pr list"]));
    let error = world.afk(&["plan"]).error();
    assert!(error.contains("gh pr list failed"), "{error}");
}

#[test]
fn a_pr_closes_what_its_body_says_whatever_github_linked() {
    let world = World::new();
    world.issue(1, &["ready-for-agent"]);
    world.issue(2, &["ready-for-agent"]);
    world.pr(30, 1, "feature-1");
    world.with_gh(|gh| {
        gh["prs"][0]["body"] = json!("Work.\n\nFixes #1 and `closes #2`");
    });
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(numbers(&plan["frontier"]), [2]);
}

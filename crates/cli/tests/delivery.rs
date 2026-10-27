//! Delivery creation, task routing and human review use the real CLI and Git.
use crate::common::{World, commit, git};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

const JIRA: &str = "https://example.atlassian.net/browse/ABC-123";

fn create(world: &World, issue: Option<&str>) -> Value {
    let mut args = vec!["delivery", "--jira", JIRA, "--final-target", "main"];
    if let Some(issue) = issue {
        args.extend(["--issue", issue]);
    }
    world.afk(&args).ok()
}

fn final_prs(world: &World) -> Vec<Value> {
    world.gh()["prs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["headRefName"].as_str().unwrap().starts_with("delivery/jira-"))
        .cloned()
        .collect()
}

fn advance(world: &World, delivery: &Value) {
    commit(&world.clone, "contribution.txt", "meaningful contribution\n");
    git(
        &world.clone,
        &["push", "-q", "origin", &format!("HEAD:{}", delivery["branch"].as_str().unwrap())],
    );
}

#[test]
fn first_task_lands_only_on_delivery_and_opens_one_human_owned_draft() {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &[]);
    let main = world.remote(&["rev-parse", "refs/heads/main"]);
    let delivery = create(&world, Some("7"));
    assert_eq!(delivery["tasks"], json!(["#7"]));
    assert!(delivery["final_pr"]["pending"].as_str().unwrap().contains("no meaningful diff"));
    let branch = delivery["branch"].as_str().unwrap();
    assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{branch}")]), main);
    let record: Value =
        serde_json::from_str(&world.remote(&["show", "-s", "--format=%s", "refs/convoy/target/7"]))
            .unwrap();
    assert!(record.get("branch").is_none(), "child stores a reference, not branch settings");
    assert_eq!(record["delivery"]["identity"], delivery["delivery"]);
    assert!(record["delivery"]["record"].is_string());
    let started = world
        .afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1", "--now", "1000"])
        .ok();
    let dir = PathBuf::from(started["worktree"].as_str().unwrap());
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let brief = std::fs::read_to_string(notes.join("convoy-worker-prompt.md")).unwrap();
    assert!(brief.contains(&format!("gh pr create --base {branch}")));
    assert!(brief.contains(&format!("Delivery: {}", delivery["delivery"].as_str().unwrap())));
    commit(&dir, "work.txt", "task seven\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, started["branch"].as_str().unwrap());
    world.with_gh(|gh| gh["prs"][0]["baseRefName"] = json!(branch));
    let landed = world.afk_in(&dir, &["land", "--issue", "7"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(landed["target_branch"], branch);
    assert!(landed.get("delivery_error").is_none(), "{landed}");
    assert_eq!(landed["final_pr"]["draft"], true);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), main);
    assert_eq!(world.remote(&["show", &format!("refs/heads/{branch}:work.txt")]), "task seven");
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.state(8), "open");
    let prs = final_prs(&world);
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0]["baseRefName"], "main");
    assert!(prs[0]["body"].as_str().unwrap().contains(JIRA));
    assert!(!prs[0]["body"].as_str().unwrap().contains("Closes"));
    assert!(
        world.gh()["comments"]["70"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["body"].as_str().unwrap().contains("convoy:task-delivery"))
    );
    let attached = create(&world, Some("8"));
    assert_eq!(attached["final_pr"], landed["final_pr"]);
    assert_eq!(attached["tasks"], json!(["#7", "#8"]));
    assert_eq!(world.afk(&["target", "--issue", "8"]).ok()["target_branch"], branch);
    // A final PR must stay outside dispatch and every ordinary PR lookup,
    // even if somebody adds a closing relationship to it.
    world.with_gh(|gh| {
        let pr = gh["prs"].as_array_mut().unwrap().iter_mut().find(|p| p["draft"] == true).unwrap();
        pr["body"] = json!(format!("{}\nCloses #8", pr["body"].as_str().unwrap()));
        pr["linked"] = json!([8]);
    });
    let read = world.afk(&["scan"]).ok();
    assert!(!read.to_string().contains("Jira delivery:"), "{read}");
    let refused = world.afk(&["land", "--issue", "8"]).error();
    assert!(refused.contains("no open PR"), "{refused}");
    assert_eq!(final_prs(&world).len(), 1);
    let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md")).unwrap();
    assert!(config.contains("base_branch: main"));
}

#[test]
fn site_context_and_code_repo_distinguish_identity_and_invalid_requests_write_nothing() {
    let world = World::new();
    let first = create(&world, None);
    for jira in [
        "https://another.atlassian.net/browse/ABC-123",
        "https://example.atlassian.net/jira/browse/ABC-123",
    ] {
        let other = world.afk(&["delivery", "--jira", jira, "--final-target", "main"]).ok();
        assert_ne!(other["delivery"], first["delivery"]);
        assert_ne!(other["branch"], first["branch"]);
    }
    let normalized = world
        .afk(&[
            "delivery",
            "--jira",
            "https://EXAMPLE.atlassian.net/browse/abc-123",
            "--final-target",
            "main",
        ])
        .ok();
    assert_eq!(normalized["delivery"], first["delivery"]);
    let another = World::new();
    another.with_gh(|gh| gh["repository"] = json!("other/repo"));
    assert_ne!(create(&another, None)["delivery"], first["delivery"]);
    let refs = world.remote(&["show-ref"]);
    for jira in [
        "ABC-123",
        "https://user:secret@example.com/browse/ABC-123",
        "https://example.com/browse/ABC-123?x=1",
        "https://example.com/../browse/ABC-123",
        "https://example.com/browse/not-a-key",
    ] {
        let refused = world.afk(&["delivery", "--jira", jira, "--final-target", "main"]).error();
        assert!(refused.contains("full Jira reference"), "{refused}");
    }
    world
        .afk(&[
            "delivery",
            "--jira",
            "https://new.example.com/browse/ABC-123",
            "--final-target",
            "missing",
        ])
        .error();
    world.afk(&["delivery", "--jira", JIRA, "--final-target", "conflicting"]).error();
    assert_eq!(world.remote(&["show-ref"]), refs);
    assert!(final_prs(&world).is_empty());
}

#[test]
fn identical_unowned_branch_is_refused_and_atomic_creation_leaves_no_record() {
    let world = World::new();
    let template = create(&world, None);
    let name = template["record"].as_str().unwrap();
    let branch = template["branch"].as_str().unwrap();
    // Simulate an unrelated branch at exactly the expected starting commit.
    git(&world.clone, &["push", "-q", "origin", &format!(":{name}")]);
    let before = world.remote(&["show-ref"]);
    let refused = world.afk(&["delivery", "--jira", JIRA, "--final-target", "main"]).error();
    assert!(refused.contains("unrelated branch"), "{refused}");
    assert_eq!(world.remote(&["show-ref"]), before);
    assert!(world.remote(&["rev-parse", &format!("refs/heads/{branch}")]).len() >= 40);
}

#[test]
fn concurrent_requests_reuse_delivery_and_single_final_pr_even_with_a_lost_receipt() {
    let world = World::new();
    let requests: Vec<Value> = std::thread::scope(|scope| {
        let joins: Vec<_> = (0..3).map(|_| scope.spawn(|| create(&world, None))).collect();
        joins.into_iter().map(|join| join.join().unwrap()).collect()
    });
    assert!(requests.iter().all(|r| r["delivery"] == requests[0]["delivery"]));
    assert_eq!(
        world
            .remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/delivery/"])
            .lines()
            .count(),
        1
    );
    advance(&world, &requests[0]);
    world.with_gh(|gh| gh["lose_pr_create_receipt"] = json!(true));
    let reviews: Vec<Value> = std::thread::scope(|scope| {
        let joins: Vec<_> = (0..3).map(|_| scope.spawn(|| create(&world, None))).collect();
        joins.into_iter().map(|join| join.join().unwrap()).collect()
    });
    assert_eq!(final_prs(&world).len(), 1);
    assert!(reviews.iter().all(|r| r["final_pr"] == reviews[0]["final_pr"]));
    let id = requests[0]["delivery"].as_str().unwrap();
    // Lose the local/remote receipt after GitHub accepted the PR. Recovery
    // discovers all PR states and records the same PR, including when closed.
    git(&world.clone, &["push", "-q", "origin", &format!(":refs/convoy/delivery-pr/{id}")]);
    world.with_gh(|gh| gh["prs"][0]["state"] = json!("closed"));
    let recovered = create(&world, None);
    assert_eq!(recovered["final_pr"]["number"], reviews[0]["final_pr"]["number"]);
    assert_eq!(recovered["final_pr"]["state"], "closed");
    assert_eq!(final_prs(&world).len(), 1);
}

#[test]
fn interrupted_atomic_creation_recovers_and_inspection_never_attempts_an_empty_pr() {
    let world = World::new();
    let marker = world.root.join("published");
    let hook = world.bare.join("hooks/post-receive");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\nsleep 1\n", marker.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["delivery", "--jira", JIRA, "--final-target", "main"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < until, "creation did not publish");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_file(hook).unwrap();
    let recovered = create(&world, None);
    let inspected = world.afk(&["delivery", "--jira", JIRA]).ok();
    assert_eq!(inspected["delivery"], recovered["delivery"]);
    assert_eq!(inspected["final_pr"], recovered["final_pr"]);
    create(&world, None);
    assert_eq!(
        world.gh()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|call| {
                call.as_array().unwrap().iter().take(2).cloned().collect::<Vec<_>>()
                    == vec![json!("pr"), json!("create")]
            })
            .count(),
        0
    );
    assert_eq!(
        world.remote(&[
            "rev-parse",
            &format!("refs/heads/{}", recovered["branch"].as_str().unwrap())
        ]),
        world.remote(&["rev-parse", "refs/heads/main"])
    );
}

#[test]
fn missing_replaced_or_conflicting_delivery_metadata_blocks_tasks_without_fallback() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let delivery = create(&world, Some("7"));
    let name = delivery["record"].as_str().unwrap();
    let original = world.remote(&["rev-parse", name]);
    git(&world.clone, &["push", "-q", "origin", &format!(":{name}")]);
    let read = world.afk(&["target", "--issue", "7"]).error();
    assert!(read.contains("missing or contradictory delivery"), "{read}");
    let refused = world.afk(&["dispatch", "--issue", "7", "--instance", "alpha"]).error();
    assert!(refused.contains("delivery"), "{refused}");
    assert!(world.claims().is_empty());
    git(&world.clone, &["push", "-q", "origin", &format!("{original}:{name}")]);
    let subject = world.remote(&["show", "-s", "--format=%s", name]);
    let mut changed: Value = serde_json::from_str(&subject).unwrap();
    changed["final_target"] = json!("other");
    let tree = git(&world.clone, &["rev-parse", "HEAD^{tree}"]);
    let replacement = git(&world.clone, &["commit-tree", &tree, "-m", &changed.to_string()]);
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{replacement}:{name}")]);
    assert!(world.afk(&["target", "--issue", "7"]).error().contains("contradictory delivery"));
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{original}:{name}")]);
    world.issue(8, &[]);
    assert!(
        world
            .afk(&["target", "--issue", "8", "--branch", delivery["branch"].as_str().unwrap()])
            .error()
            .contains("delivery reference")
    );
    assert_eq!(world.afk(&["target", "--issue", "7"]).ok()["target_branch"], delivery["branch"]);
}

#[test]
fn acting_tick_recovers_final_review_and_ambiguous_or_edited_review_refuses_duplication() {
    let world = World::new();
    world.set_gate("true");
    let delivery = create(&world, None);
    advance(&world, &delivery);
    // Simulate a crash after task publication: a plan cannot create final review.
    world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert!(final_prs(&world).is_empty());
    let ticked = world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert_eq!(final_prs(&world).len(), 1);
    let pr = final_prs(&world)[0].clone();
    world.with_gh(|gh| {
        let mut duplicate = pr.clone();
        duplicate["number"] = json!(999);
        gh["prs"].as_array_mut().unwrap().push(duplicate);
    });
    assert!(
        world
            .afk(&["delivery", "--jira", JIRA, "--final-target", "main"])
            .error()
            .contains("ambiguous final PR")
    );
    world.with_gh(|gh| {
        gh["prs"].as_array_mut().unwrap().pop();
        gh["prs"][0]["baseRefName"] = json!("other");
    });
    assert!(
        world
            .afk(&["delivery", "--jira", JIRA, "--final-target", "main"])
            .error()
            .contains("conflicts with delivery")
    );
    assert_eq!(final_prs(&world).len(), 1);
}

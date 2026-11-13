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

#[test]
fn branch_layout_and_separate_tracker_keep_one_authoritative_delivery() {
    let world = World::new();
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, "acme/tracker");
    world.with_tracker(|gh| {
        gh["issues"].as_array_mut().unwrap().push(json!({
            "number": 7, "title": "first task", "labels": ["repo:acme/dev"], "state": "open"
        }))
    });
    afkconvoy::git::Repo::new(world.clone.clone())
        .note_layout(afkconvoy_core::layout::Layout::Branches)
        .unwrap();
    let delivery = create(&world, Some("7"));
    assert_eq!(delivery["tasks"], json!(["acme/tracker#7"]));
    assert!(delivery["record"].as_str().unwrap().starts_with("refs/heads/convoy-delivery/"));
    let target: Value = serde_json::from_str(&world.remote(&[
        "show",
        "-s",
        "--format=%s",
        "refs/heads/convoy-target/acme/tracker/7",
    ]))
    .unwrap();
    assert_eq!(target["delivery"]["identity"], delivery["delivery"]);
    assert!(target.get("branch").is_none());
    let read = world.afk(&["target", "--issue", "7"]).ok();
    assert_eq!(read["target_branch"], delivery["branch"]);
    git(
        &world.clone,
        &["push", "-q", "origin", &format!(":{}", delivery["branch"].as_str().unwrap())],
    );
    assert!(world.afk(&["target", "--issue", "7"]).error().contains("missing"));
}

#[test]
fn rest_exhaustion_preserves_final_review_discovery_and_watch_exclusion() {
    let world = World::new();
    world.list_checkout(&world.clone);
    let delivery = create(&world, None);
    advance(&world, &delivery);
    world.with_gh(|gh| gh["quota_fail"] = json!({"core": "gh: API rate limit already exceeded"}));
    let created = create(&world, None);
    assert_eq!(created["final_pr"]["draft"], true);
    assert_eq!(final_prs(&world).len(), 1);
    assert_eq!(create(&world, None)["final_pr"], created["final_pr"]);
    world.pr(900, 7, "ordinary-task");
    world.with_gh(|gh| {
        for pr in gh["prs"].as_array_mut().unwrap() {
            pr["updated_at"] = json!(1050);
            pr["closed_at"] = json!(1050);
            pr["state"] = json!("closed");
        }
        gh["prs"][1]["body"] = json!("");
    });
    let watched = world.afk(&["doctor", "--all", "--since", "1000", "--now", "1100"]).ok();
    let ended = watched["checkouts"][0]["doctor"]["watching"]["ended"].as_array().unwrap();
    assert_eq!(ended.len(), 1, "ordinary task endings remain visible: {watched}");
    assert_eq!(ended[0]["pr"], 900);
    assert!(!watched.to_string().contains("Jira delivery:"), "{watched}");
    assert_eq!(create(&world, None)["final_pr"]["state"], "closed");
}

/// Execute the creation line exactly as a discussion agent receives it.
fn open_from_flows(world: &World, flows: &Value) -> u64 {
    let body = world.root.join("task.md");
    std::fs::write(&body, "A contribution discussed for this Jira delivery.\n").unwrap();
    let script = flows["line"]
        .as_str()
        .unwrap()
        .replace("'<title>'", "'Discussed task'")
        .replace("<file>", "\"$1\"");
    let output = world
        .command("sh", &world.clone)
        .args(["-c", &script, "sh", body.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().rsplit('/').next().unwrap().parse().unwrap()
}

#[test]
fn discussion_creation_reconstructs_three_tasks_and_lands_only_on_their_two_deliveries() {
    let world = World::new();
    world.set_gate("true");
    world.list_checkout(&world.clone);
    let first = create(&world, None);
    let second = world
        .afk(&[
            "delivery",
            "--jira",
            "https://example.atlassian.net/browse/ABC-456",
            "--final-target",
            "main",
        ])
        .ok();
    let main = world.remote(&["rev-parse", "refs/heads/main"]);
    let mut tasks = vec![];
    for delivery in [&first, &first, &second] {
        let id = delivery["delivery"].as_str().unwrap();
        let flows = world.afk(&["flows", "--delivery", id]).ok();
        assert!(!flows["line"].as_str().unwrap().contains("ready-for-agent"));
        let issue = open_from_flows(&world, &flows);
        assert!(!world.labels(issue).contains(&"ready-for-agent".to_string()));
        let script = flows["intake_line"]
            .as_str()
            .unwrap()
            .replace("<n>", &issue.to_string())
            .replacen("afkconvoy", "\"$1\"", 1);
        let output = world
            .command("sh", &world.clone)
            .args(["-c", &script, "sh", env!("CARGO_BIN_EXE_afkconvoy")])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        assert!(world.labels(issue).contains(&"ready-for-agent".to_string()));
        tasks.push(issue);
    }
    for _ in 0..2 {
        assert_eq!(
            world.afk(&["delivery", "--jira", JIRA]).ok()["tasks"],
            json!(tasks[..2].iter().map(|n| format!("#{n}")).collect::<Vec<_>>())
        );
    }
    let targets = world.afk(&["plan"]).ok()["targets"].clone();
    assert_eq!(targets.as_array().unwrap().len(), 3);
    let doctor = world.afk(&["doctor"]).ok();
    assert_eq!(doctor["targets"], targets);
    for (at, issue) in tasks.iter().enumerate() {
        let delivery = if at < 2 { &first } else { &second };
        let branch = delivery["branch"].as_str().unwrap();
        let number = issue.to_string();
        assert_eq!(
            world.afk(&["target", "--issue", &number]).ok()["delivery"],
            delivery["delivery"]
        );
        let started = world
            .afk(&["dispatch", "--issue", &number, "--instance", "alpha", "--host", "h1"])
            .ok();
        let dir = PathBuf::from(started["worktree"].as_str().unwrap());
        let file = format!("task-{issue}.txt");
        commit(&dir, &file, &number);
        git(&dir, &["push", "-q", "origin", "HEAD"]);
        let pr = 2000 + issue * 10;
        world.pr(pr, *issue, started["branch"].as_str().unwrap());
        world.with_gh(|gh| {
            let row =
                gh["prs"].as_array_mut().unwrap().iter_mut().find(|p| p["number"] == pr).unwrap();
            row["baseRefName"] = json!(branch);
        });
        let landed = world.afk_in(&dir, &["land", "--issue", &number]).ok();
        assert_eq!(landed["outcome"], "merged", "{landed}");
        assert_eq!(landed["target_branch"], branch);
        assert_eq!(world.remote(&["show", &format!("refs/heads/{branch}:{file}")]), number);
        let other = if at < 2 { &second } else { &first };
        let absent = std::process::Command::new("git")
            .args([
                "--git-dir",
                world.bare.to_str().unwrap(),
                "cat-file",
                "-e",
                &format!("refs/heads/{}:{file}", other["branch"].as_str().unwrap()),
            ])
            .output()
            .unwrap();
        assert!(!absent.status.success());
    }
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), main);
    assert_eq!(final_prs(&world).len(), 2);
    world.issue(99, &[]);
    world.afk(&["intake", "--issue", "99"]).ok();
    assert_eq!(world.afk(&["target", "--issue", "99"]).ok()["target_branch"], "main");
    assert!(world.afk(&["target", "--issue", "99"]).ok().get("delivery").is_none());
}

#[test]
fn invalid_context_and_existing_evidence_refuse_before_a_runnable_label() {
    let world = World::new();
    world.issue(7, &[]);
    world.issue(8, &[]);
    world.list_checkout(&world.clone);
    let delivery = create(&world, None);
    let id = delivery["delivery"].as_str().unwrap();
    for invalid in ["missing", "0000000000000000"] {
        world.afk(&["flows", "--delivery", invalid]).error();
        world.afk(&["intake", "--issue", "7", "--delivery", invalid]).error();
        assert!(world.labels(7).is_empty());
        assert!(world.claims().is_empty());
    }
    world.with_gh(|gh| gh["repository"] = json!("another/repo"));
    assert!(
        world
            .afk(&["intake", "--issue", "7", "--delivery", id])
            .error()
            .contains("belongs to code repo")
    );
    assert!(world.labels(7).is_empty());
    world.with_gh(|gh| gh["repository"] = json!("acme/dev"));
    // An unaccounted PR must not acquire a new destination just because its
    // target record was never written (for example by an older worker).
    world.pr(70, 7, "earlier-work");
    let error = world.afk(&["intake", "--issue", "7", "--delivery", id]).error();
    assert!(error.contains("PRs [70]"), "{error}");
    assert!(world.labels(7).is_empty());
    world.afk(&["claim", "--issue", "8", "--instance", "alpha"]).ok();
    let error = world.afk(&["intake", "--issue", "8", "--delivery", id]).error();
    assert!(error.contains("pinned") && error.contains("main"), "{error}");
    assert!(world.labels(8).is_empty());
    assert_eq!(world.gh()["prs"][0]["state"], "open");
    // An absent definition remains a refusal on every fresh invocation.
    git(
        &world.clone,
        &["push", "-q", "origin", &format!(":{}", delivery["record"].as_str().unwrap())],
    );
    world.issue(9, &[]);
    assert!(
        world
            .afk(&["intake", "--issue", "9", "--delivery", id])
            .error()
            .contains("missing delivery")
    );
    assert!(world.labels(9).is_empty());
}

fn handoff(world: &World) -> Value {
    world.afk(&["delivery", "--jira", JIRA, "--handoff"]).ok()
}

fn refs_under(world: &World, prefix: &str) -> Vec<String> {
    world
        .remote(&["for-each-ref", "--format=%(refname)", prefix])
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn tip(world: &World, branch: &str) -> String {
    world.remote(&["rev-parse", &format!("refs/heads/{branch}")])
}

/// Lands one attached task on the delivery branch and lets the fleet settle
/// the worker's claim, which is what leaves the task implemented.
fn contribute(world: &World, delivery: &Value, issue: u64, file: &str, text: &str, now: u64) {
    let number = issue.to_string();
    let started = world
        .afk(&[
            "dispatch",
            "--issue",
            &number,
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            &now.to_string(),
        ])
        .ok();
    let dir = PathBuf::from(started["worktree"].as_str().unwrap());
    commit(&dir, file, text);
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    let branch = started["branch"].as_str().unwrap().to_string();
    world.pr(issue * 10, issue, &branch);
    let target = delivery["branch"].as_str().unwrap().to_string();
    world.with_gh(|gh| {
        let pr = gh["prs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|pr| pr["number"] == json!(issue * 10))
            .unwrap();
        pr["baseRefName"] = json!(target);
    });
    let landed = world.afk_in(&dir, &["land", "--issue", &number]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(landed["target_branch"], delivery["branch"]);
    world
        .afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &(now + 1000).to_string()])
        .ok();
}

fn two_tasks(world: &World) -> Value {
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    let delivery = create(world, Some("7"));
    contribute(world, &delivery, 7, "seven.txt", "task seven\n", 1000);
    world.issue(8, &["ready-for-agent"]);
    create(world, Some("8"));
    contribute(world, &delivery, 8, "eight.txt", "task eight\n", 3000);
    delivery
}

fn one_task(world: &World) -> Value {
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    let delivery = create(world, Some("7"));
    contribute(world, &delivery, 7, "seven.txt", "task seven\n", 1000);
    delivery
}

#[test]
fn two_tasks_hand_off_one_pr_and_a_human_merge_is_reconstructed_after_restart() {
    let world = World::new();
    let delivery = two_tasks(&world);
    let branch = delivery["branch"].as_str().unwrap().to_string();
    let main = tip(&world, "main");
    assert_eq!(final_prs(&world).len(), 1);
    assert_eq!(final_prs(&world)[0]["draft"], true);
    assert!(refs_under(&world, "refs/convoy/delivery-evidence").is_empty());
    assert!(
        !refs_under(&world, "refs/convoy/gate").is_empty(),
        "each task landing records its own green gate"
    );

    world.gh_calls();
    let handed = handoff(&world);
    assert_eq!(handed["ready"], true, "{handed}");
    assert_eq!(handed["delivered"], false, "{handed}");
    assert_eq!(handed["reviews"], "not changed");
    assert_eq!(handed["gate"]["status"], "green", "{handed}");
    assert_eq!(handed["reasons"], json!([]), "{handed}");
    assert_eq!(tip(&world, "main"), main, "handoff does not publish to the final target");
    assert_eq!(final_prs(&world).len(), 1);
    assert_eq!(final_prs(&world)[0]["draft"], false);
    let body = final_prs(&world)[0]["body"].as_str().unwrap().to_string();
    assert!(body.contains("<!--convoy:delivery"), "{body}");
    assert!(body.contains(JIRA), "{body}");
    assert!(body.contains("#7 implemented"), "{body}");
    assert!(body.contains("#8 implemented"), "{body}");
    assert!(body.contains("Delivery head:"), "{body}");
    assert!(body.contains("Gate tree:"), "{body}");
    assert_eq!(refs_under(&world, "refs/convoy/delivery-evidence").len(), 1);
    let calls = world.gh_calls();
    assert!(
        calls.iter().flatten().all(|arg| arg != "review" && !arg.contains("dismiss")),
        "reviews stay as they are: {calls:?}"
    );

    let again = handoff(&world);
    assert_eq!(again["ready"], true, "{again}");
    assert_eq!(final_prs(&world).len(), 1);
    assert_eq!(refs_under(&world, "refs/convoy/delivery-evidence").len(), 1);
    assert_eq!(tip(&world, "main"), main);

    world.issue(9, &["ready-for-agent"]);
    world.with_gh(|gh| {
        let pr = gh["prs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|pr| pr["headRefName"].as_str().unwrap().starts_with("delivery/jira-"))
            .unwrap();
        let body = pr["body"].as_str().unwrap().to_string();
        pr["body"] = json!(format!("{body}\nCloses #9\n"));
        pr["linked"] = json!([9]);
    });
    let refused = world.afk(&["land", "--issue", "9"]).error();
    assert!(refused.contains("no open PR"), "{refused}");
    assert_eq!(tip(&world, "main"), main);
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "5000"]).ok();
    assert_eq!(tip(&world, "main"), main, "a tick does not land the final PR");

    let inspected = world.afk(&["delivery", "--jira", JIRA]).ok();
    assert_eq!(inspected["tasks"], json!(["#7", "#8"]));
    assert!(inspected["tasks"][0].is_string());
    assert_eq!(inspected["handoff"]["certifies"], true, "{inspected}");
    let plan = world.afk(&["plan"]).ok();
    let doctor = world.afk(&["doctor"]);
    // The standalone tick left #9 claimed, with no host runner working it.
    assert_eq!(doctor.code, 1);
    let doctor = doctor.json;
    assert!(doctor["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "no_run"));
    assert_eq!(plan["deliveries"][0]["certifies"], true, "{plan}");
    assert_eq!(plan["deliveries"][0]["detail"], "ready for human review");
    assert_eq!(doctor["deliveries"][0]["detail"], plan["deliveries"][0]["detail"]);
    // `panel` prints one HTML document, not a JSON object.
    let out =
        world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone).arg("panel").output().unwrap();
    let html = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(html.contains("Jira deliveries"), "{html}");
    assert!(html.contains(JIRA), "{html}");

    git(&world.clone, &["branch", "kept-aside"]);
    git(&world.clone, &["push", "-q", "origin", "kept-aside"]);
    let aside = tip(&world, "kept-aside");
    let delivery_tip = tip(&world, &branch);
    git(&world.clone, &["fetch", "-q", "origin", &branch]);
    git(&world.clone, &["merge", "--ff-only", &format!("origin/{branch}")]);
    git(&world.clone, &["push", "-q", "origin", "HEAD:main"]);

    let settled = handoff(&world);
    assert_eq!(settled["delivered"], true, "{settled}");
    assert_eq!(settled["ready"], false, "{settled}");
    assert_eq!(settled["discrepancy"], false, "{settled}");
    assert_eq!(settled["settlement"]["complete"], true, "{settled}");
    assert_eq!(settled["settlement"]["observed"], "merged");
    assert_eq!(settled["reviews"], "not changed");
    let settlement = refs_under(&world, "refs/convoy/delivery-settled");
    assert_eq!(settlement.len(), 1, "{settlement:?}");
    let reread = handoff(&world);
    assert_eq!(reread["delivered"], true, "{reread}");
    assert_eq!(refs_under(&world, "refs/convoy/delivery-settled"), settlement);
    assert_eq!(tip(&world, &branch), delivery_tip);
    assert_eq!(tip(&world, "kept-aside"), aside);
    assert_eq!(world.remote(&["show", "refs/heads/main:seven.txt"]), "task seven");
    assert_eq!(world.remote(&["show", "refs/heads/main:eight.txt"]), "task eight");
}

#[test]
fn a_late_task_withdraws_readiness_and_old_evidence_does_not_certify() {
    let world = World::new();
    let delivery = two_tasks(&world);
    let branch = delivery["branch"].as_str().unwrap().to_string();
    let main = tip(&world, "main");
    let delivery_tip = tip(&world, &branch);
    let ready = handoff(&world);
    assert_eq!(ready["ready"], true, "{ready}");
    let evidence = refs_under(&world, "refs/convoy/delivery-evidence");

    world.issue(9, &[]);
    create(&world, Some("9"));
    let reported = world.afk(&["doctor"]).ok();
    assert_eq!(reported["deliveries"][0]["certifies"], false, "{reported}");
    assert_eq!(reported["deliveries"][0]["detail"], "not ready");
    let withdrawn = handoff(&world);
    assert_eq!(withdrawn["ready"], false, "{withdrawn}");
    let reasons = withdrawn["reasons"].to_string();
    assert!(reasons.contains("#9"), "{reasons}");
    assert!(reasons.contains("still open"), "{reasons}");
    assert_eq!(final_prs(&world)[0]["draft"], true);
    assert_eq!(tip(&world, "main"), main);
    assert_eq!(tip(&world, &branch), delivery_tip);
    assert_eq!(refs_under(&world, "refs/convoy/delivery-evidence"), evidence);
    let inspected = world.afk(&["delivery", "--jira", JIRA]).ok();
    assert_eq!(inspected["tasks"], json!(["#7", "#8", "#9"]));
    assert_eq!(inspected["handoff"]["certifies"], false, "{inspected}");
}

#[test]
fn a_moved_final_target_with_a_red_gate_preserves_both_branches() {
    let world = World::new();
    let delivery = two_tasks(&world);
    let branch = delivery["branch"].as_str().unwrap().to_string();
    let ready = handoff(&world);
    assert_eq!(ready["ready"], true, "{ready}");
    let delivery_tip = tip(&world, &branch);
    world.set_gate("false");
    let main = tip(&world, "main");
    let red = handoff(&world);
    assert_eq!(red["ready"], false, "{red}");
    assert_eq!(red["gate"]["status"], "red", "{red}");
    assert_eq!(final_prs(&world)[0]["draft"], true);
    assert_eq!(tip(&world, &branch), delivery_tip);
    assert_eq!(tip(&world, "main"), main, "a red gate does not move the final target back");
    assert_ne!(tip(&world, "main"), delivery_tip);
    let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md")).unwrap();
    assert!(config.contains("gate_command: \"false\""), "{config}");
    let reported = world.afk(&["doctor"]).ok();
    assert_eq!(reported["deliveries"][0]["certifies"], false, "{reported}");
}

#[test]
fn a_conflict_with_the_final_target_leaves_both_branches_and_withdraws() {
    let world = World::new();
    let delivery = one_task(&world);
    let branch = delivery["branch"].as_str().unwrap().to_string();
    let ready = handoff(&world);
    assert_eq!(ready["ready"], true, "{ready}");
    let delivery_tip = tip(&world, &branch);
    commit(&world.clone, "seven.txt", "from the final target\n");
    git(&world.clone, &["push", "-q", "origin", "HEAD:main"]);
    let main = tip(&world, "main");
    let conflict = handoff(&world);
    assert_eq!(conflict["ready"], false, "{conflict}");
    assert_eq!(conflict["gate"]["status"], "conflict", "{conflict}");
    assert!(conflict["reasons"].to_string().contains("conflicts"), "{conflict}");
    assert_eq!(final_prs(&world)[0]["draft"], true);
    assert_eq!(tip(&world, &branch), delivery_tip);
    assert_eq!(tip(&world, "main"), main);
    assert_eq!(world.remote(&["show", &format!("refs/heads/{branch}:seven.txt")]), "task seven");
}

#[test]
fn a_closed_unmerged_pr_is_observed_once_and_needs_action() {
    let world = World::new();
    let delivery = one_task(&world);
    let branch = delivery["branch"].as_str().unwrap().to_string();
    let delivery_tip = tip(&world, &branch);
    world.with_gh(|gh| {
        let pr = gh["prs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|pr| pr["headRefName"].as_str().unwrap().starts_with("delivery/jira-"))
            .unwrap();
        pr["state"] = json!("closed");
        pr["merged"] = json!(false);
    });
    let observed = handoff(&world);
    assert_eq!(observed["delivered"], false, "{observed}");
    assert_eq!(observed["settlement"]["observed"], "closed", "{observed}");
    assert_eq!(observed["settlement"]["complete"], false, "{observed}");
    assert!(observed["reasons"].to_string().contains("needs action"), "{observed}");
    let settlement = refs_under(&world, "refs/convoy/delivery-settled");
    assert_eq!(settlement.len(), 1, "{settlement:?}");
    let again = handoff(&world);
    assert_eq!(again["delivered"], false, "{again}");
    assert_eq!(refs_under(&world, "refs/convoy/delivery-settled"), settlement);
    assert_eq!(tip(&world, &branch), delivery_tip);
    let reported = world.afk(&["plan"]).ok();
    assert_eq!(reported["deliveries"][0]["detail"], "final PR closed unmerged; needs action");
}

#[test]
fn a_merged_flag_without_the_commit_on_the_target_is_not_delivery() {
    let world = World::new();
    let delivery = one_task(&world);
    let main = tip(&world, "main");
    world.with_gh(|gh| {
        let pr = gh["prs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|pr| pr["headRefName"].as_str().unwrap().starts_with("delivery/jira-"))
            .unwrap();
        pr["state"] = json!("merged");
        pr["merged"] = json!(true);
    });
    let observed = handoff(&world);
    assert_eq!(observed["delivered"], false, "{observed}");
    assert_eq!(observed["settlement"]["complete"], false, "{observed}");
    assert_eq!(observed["settlement"]["on_target"], false, "{observed}");
    assert!(
        observed["reasons"].to_string().contains("not on the declared final target"),
        "{observed}"
    );
    assert_eq!(tip(&world, "main"), main);
    let settlement = refs_under(&world, "refs/convoy/delivery-settled");
    assert_eq!(handoff(&world)["delivered"], false);
    assert_eq!(refs_under(&world, "refs/convoy/delivery-settled"), settlement);
    let _ = delivery;
}

#[test]
fn a_task_closed_as_not_planned_is_not_implemented() {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    create(&world, Some("7"));
    world.close(7, "not_planned");
    let handed = handoff(&world);
    assert_eq!(handed["ready"], false, "{handed}");
    assert_eq!(handed["delivered"], false, "{handed}");
    let reasons = handed["reasons"].to_string();
    assert!(reasons.contains("not planned"), "{reasons}");
    assert!(handed["tasks"][0]["result"] == json!("not_planned"), "{handed}");
    assert!(refs_under(&world, "refs/convoy/delivery-evidence").is_empty());
    assert!(final_prs(&world).is_empty());
}

#[test]
fn a_merge_while_an_attached_task_is_open_is_not_the_whole_delivery() {
    let world = World::new();
    let delivery = one_task(&world);
    let branch = delivery["branch"].as_str().unwrap().to_string();
    world.issue(8, &[]);
    create(&world, Some("8"));
    git(&world.clone, &["fetch", "-q", "origin", &branch]);
    git(&world.clone, &["merge", "--ff-only", &format!("origin/{branch}")]);
    git(&world.clone, &["push", "-q", "origin", "HEAD:main"]);
    let observed = handoff(&world);
    assert_eq!(observed["delivered"], false, "{observed}");
    assert_eq!(observed["settlement"]["observed"], "merged", "{observed}");
    assert_eq!(observed["settlement"]["complete"], false, "{observed}");
    assert!(observed["reasons"].to_string().contains("not delivered"), "{observed}");
    let settlement = refs_under(&world, "refs/convoy/delivery-settled");
    assert_eq!(handoff(&world)["settlement"]["complete"], false);
    assert_eq!(refs_under(&world, "refs/convoy/delivery-settled"), settlement);
    assert_eq!(tip(&world, &branch), world.remote(&["rev-parse", "refs/heads/main"]));
}

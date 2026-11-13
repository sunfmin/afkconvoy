use crate::common;

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
        gh["edges"]["9"] = json!([7]); // #9 waits on #7
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
            (9, "open blockers: #7"),
        ]
    );
    assert_eq!(
        plan["mine"],
        json!([
            {"number": 1, "title": "issue 1", "status": "no_pr", "pr": null, "declared": false, "idle": false},
            {"number": 4, "title": "issue 4", "status": "landing", "pr": 40, "declared": false, "idle": true},
            {"number": 6, "title": null, "status": "closed", "pr": null, "declared": false, "idle": false},
        ])
    );
    assert_eq!(numbers(&plan["peer_live"]), [2]);
    assert_eq!(numbers(&plan["stale"]), [3]);
    assert_eq!(plan["stale"][0]["instance"], "dead");
    assert_eq!(plan["stale"][0]["sha"], world.remote(&["rev-parse", "refs/convoy/claim/3"]));
    assert_eq!(numbers(&plan["stale_closed"]), [10]);
    assert_eq!(plan["free_slots"], 1, "the idle landing #4 takes none of the two slots");
}

#[test]
fn a_pr_from_a_fork_is_read_by_nothing() {
    let world = backlog();
    world.issue(11, &["ready-for-agent"]);
    // A fork's PRs, numbered past the fleet's own: one closes #4, which is
    // joining on PR 40, one #11, unclaimed and ready.
    world.fork_pr(99, 4, "feature-4");
    world.fork_pr(98, 11, "patch-1");
    let plan = world.afk(&["plan", "--instance", "me", "--now", "10000"]).ok();
    assert_eq!(
        plan["mine"][1],
        json!({"number": 4, "title": "issue 4", "status": "landing", "pr": 40, "declared": false, "idle": true})
    );
    assert_eq!(numbers(&plan["frontier"]), [11], "{plan}");
    assert!(!numbers(&plan["excluded"]).contains(&11), "{plan}");
}

#[test]
fn an_unclaimed_ready_issue_is_on_the_frontier_in_number_order() {
    let world = World::new();
    world.list_checkout(&world.clone);
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
        assert!(
            line.starts_with("api ")
                || line.starts_with("pr list")
                || call.starts_with(&["config".into(), "get".into(), "user".into()]),
            "{line}"
        );
        // The one GraphQL call is a query: its fields say what it asks for.
        let asks = call.iter().any(|arg| arg.starts_with("query=query("));
        let writes: &[&str] =
            if asks { &["--method", "-X"] } else { &["--method", "-X", "-f", "-F"] };
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

fn plan_with_list_stalls(stalls: usize) -> (World, common::Answer) {
    let world = World::new();
    world.stall_git_lists(stalls);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("AFKCONVOY_LS_REMOTE_SECONDS", "1")
        .arg("plan")
        .output()
        .unwrap();
    let answer = common::Answer {
        json: serde_json::from_slice(&out.stdout).unwrap(),
        code: out.status.code().unwrap(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    (world, answer)
}

#[test]
fn a_remote_read_that_outlives_its_limit_is_ended_and_asked_again() {
    let (world, answer) = plan_with_list_stalls(2);
    answer.ok();
    let listings = world.git_calls().iter().filter(|call| call.starts_with("ls-remote")).count();
    assert!(listings >= 3, "the third listing answered: {listings}");
}

#[test]
fn a_remote_read_that_never_ends_fails_after_three_attempts() {
    let (world, answer) = plan_with_list_stalls(3);
    let error = answer.error();
    assert!(error.contains("3 attempts"), "{error}");
    assert!(error.contains("did not end within 1s"), "{error}");
    assert!(error.contains("its process group was ended"), "{error}");
    let listings = world.git_calls().iter().filter(|call| call.starts_with("ls-remote")).count();
    assert_eq!(listings, 3);
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

#[test]
fn plan_and_doctor_name_only_the_open_blockers_whatever_the_summary_says() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    for n in [71, 72, 73] {
        world.issue(n, &["bug"]);
    }
    world.close(72, "completed");
    world.with_gh(|gh| {
        // #74 is gone: neither API returns it, as before.
        gh["edges"]["7"] = json!([73, 72, 71, 74]);
        gh["issues"][0]["blocked_by"] = json!(0);
    });
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["frontier"], json!([]));
    assert_eq!(plan["excluded"][0]["reason"], "open blockers: #71, #73");
    let doctor = world.afk(&["doctor"]).ok();
    assert_eq!(doctor["issues"]["excluded"][0]["reason"], plan["excluded"][0]["reason"]);
}

/// Where `show <issue>` says issue `number` of `world` stands at `now`.
fn shown(world: &World, number: u64) -> Value {
    world.afk(&["show", &number.to_string(), "--now", "10000"]).ok()
}

#[test]
fn show_says_where_an_issue_of_an_unbound_repo_stands_from_what_plan_reads() {
    let world = backlog();
    world.issue(11, &["ready-for-agent"]);
    world.comment(4, "<!--convoy:branch name=feature-4-->");
    let stands = |number: u64| shown(&world, number)["stands"].clone();
    assert_eq!(stands(11), json!({"is": "ready"}));
    assert_eq!(stands(7), json!({"is": "held", "reason": "no ready-for-agent label"}));
    assert_eq!(stands(8), json!({"is": "held", "reason": "epic label (prd)"}));
    assert_eq!(stands(9), json!({"is": "held", "reason": "open blockers: #7"}));
    let five = shown(&world, 5);
    assert_eq!(five["stands"], json!({"is": "held", "reason": "has an open linked PR"}));
    assert_eq!(five["pr"], json!({"number": 50, "branch": "feature-5", "fleet": false}));
    let two = shown(&world, 2);
    assert_eq!(two["stands"], json!({"is": "claimed"}));
    assert_eq!(two["claim"], json!({"instance": "live", "live": true}));
    assert_eq!(two["pr"], Value::Null);
    assert_eq!(shown(&world, 3)["claim"], json!({"instance": "dead", "live": false}));
    // The fleet's own PR waits to land: its claim's worker lands it.
    let four = shown(&world, 4);
    assert_eq!(four["stands"], json!({"is": "landing"}));
    assert_eq!(four["pr"], json!({"number": 40, "branch": "feature-4", "fleet": true}));
    assert_eq!(four["claim"]["instance"], "me");
    assert_eq!((&four["workflow"], &four["target_branch"]), (&Value::Null, &json!("main")));
    let six = shown(&world, 6);
    assert_eq!(six["stands"]["is"], "closed", "{six}");
    assert_eq!(six["claim"], Value::Null);
    // Bare, `show` still has no workflow to draw.
    assert!(world.afk(&["show"]).error().contains("this repo is unbound"));
}

#[test]
fn show_says_a_fleet_lock_holds_an_unbound_issue_back() {
    let world = World::new();
    world.set_gate_config("gate_command: ./gate.sh\nlock_label: halt");
    world.issue(1, &["ready-for-agent"]);
    world.issue(2, &["halt"]);
    let held = shown(&world, 1)["stands"].clone();
    assert_eq!(held["is"], "held");
    assert!(held["reason"].as_str().unwrap().contains("#2"), "{held}");
}

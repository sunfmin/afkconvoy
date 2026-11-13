use crate::common::World;
use serde_json::json;

fn primary(world: &World, api: &str) {
    world.with_gh(|gh| {
        gh["quota_fail"] = json!({api: "gh: API rate limit already exceeded"});
    });
}

#[test]
fn graphql_primary_exhaustion_still_dispatches_with_authoritatively_no_open_prs() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    primary(&world, "graphql");
    let started = world
        .afk(&[
            "dispatch",
            "--issue",
            "146",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1000",
        ])
        .ok();
    assert_eq!(started["started"], true, "{started}");
}

#[test]
fn rest_primary_exhaustion_still_reads_the_frontier() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    primary(&world, "core");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 146, "{plan}");
}

fn plan(world: &World) -> serde_json::Value {
    world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok()
}

fn resource_calls(world: &World, api: &str) -> usize {
    let calls: Vec<Vec<String>> = serde_json::from_value(world.gh()["calls"].clone()).unwrap();
    world.gh_calls();
    calls
        .iter()
        .filter(|call| {
            let gql = call.starts_with(&["api".into(), "graphql".into()])
                || call.starts_with(&["pr".into(), "list".into()]);
            let core = call.first().is_some_and(|s| s == "api")
                && call.get(1).is_none_or(|s| s != "graphql" && s != "rate_limit");
            if api == "graphql" { gql } else { core }
        })
        .count()
}

fn quota_files(world: &World) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries {
                let p = e.unwrap().path();
                if p.is_dir() { walk(&p, out) } else { out.push(p) }
            }
        }
    }
    let mut files = vec![];
    walk(&world.cache().join("afkconvoy/github-quota"), &mut files);
    files
}

#[test]
fn exhausted_api_is_skipped_across_commands_and_recovers_after_reset() {
    for api in ["core", "graphql"] {
        let world = World::new();
        world.issue(146, &["ready-for-agent"]);
        primary(&world, api);
        assert_eq!(plan(&world)["frontier"][0]["number"], 146);
        assert!(resource_calls(&world, api) > 0, "initial failure must be exercised");
        plan(&world);
        assert_eq!(
            resource_calls(&world, api),
            0,
            "known exhaustion must not hit the API on another command"
        );
        // Expire the observation rather than exhausting an account or sleeping.
        for file in quota_files(&world) {
            if file.file_name().unwrap() == format!("{api}.json").as_str() {
                std::fs::write(file, r#"{"remaining":0,"reset":1}"#).unwrap();
            }
        }
        world.with_gh(|gh| gh["quota_fail"] = json!({}));
        plan(&world);
        if api == "core" {
            world.afk(&["doctor", "--now", "1000"]);
        }
        assert!(
            resource_calls(&world, api) > 0,
            "reset must allow the preferred API without a restart"
        );
    }
}

#[test]
fn known_quota_exhaustion_is_routed_before_the_first_operation() {
    for api in ["core", "graphql"] {
        let world = World::new();
        world.issue(146, &["ready-for-agent"]);
        primary(&world, api);
        world.with_gh(|gh| {
            gh["quota"] = json!({"resources":{
            "core":{"remaining":if api=="core"{0}else{50},"reset":4_000_000_000u64},
            "graphql":{"remaining":if api=="graphql"{0}else{50},"reset":4_000_000_000u64}}})
        });
        assert_eq!(plan(&world)["frontier"][0]["number"], 146);
        assert_eq!(resource_calls(&world, api), 0);
    }
}

#[test]
fn both_quotas_stop_with_reset_information_and_no_repeated_requests() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh|gh["quota_fail"]=json!({"core":"gh: API rate limit exceeded", "graphql":"GraphQL: API rate limit already exceeded"}));
    let error = world.afk(&["plan"]).error();
    assert!(
        error.contains("remaining=Some(0)")
            && error.contains("reset=")
            && error.contains("core")
            && error.contains("GraphQL"),
        "{error}"
    );
    world.gh_calls();
    assert!(world.afk(&["plan"]).error().contains("reset"));
    assert_eq!(resource_calls(&world, "core"), 0);
    assert_eq!(resource_calls(&world, "graphql"), 0);
}

#[test]
fn native_manual_link_without_body_never_becomes_duplicate_dispatch() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.pr(70, 146, "work");
    world.with_gh(|gh| {
        gh["prs"][0]["body"] = json!("No closing keyword.");
        gh["prs"][0]["linked"] = json!([146]);
    });
    assert_eq!(plan(&world)["frontier"], json!([]));
    primary(&world, "graphql");
    let error = world
        .afk(&[
            "dispatch",
            "--issue",
            "146",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1000",
        ])
        .error();
    assert!(
        error.contains("native/manual")
            && error.contains("closingIssuesReferences")
            && error.contains("reset"),
        "{error}"
    );
    assert!(
        world
            .orca_calls()
            .iter()
            .all(|call| !call.starts_with(&["terminal".into(), "create".into()]))
    );
}

#[test]
fn secondary_limits_hold_both_apis_and_honor_retry_after() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh| {
        gh["quota_fail"] = json!({"graphql":"gh: secondary rate limit\nRetry-After: 120"})
    });
    let before =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let error = world.afk(&["plan"]).error();
    assert!(error.contains("secondary rate limit"), "{error}");
    let file = quota_files(&world)
        .into_iter()
        .find(|p| p.file_name().unwrap() == "secondary.json")
        .unwrap();
    let held: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert!(held["reset"].as_u64().unwrap() >= before + 120);
    world.gh_calls();
    assert!(world.afk(&["plan"]).error().contains("both APIs wait"));
    assert_eq!(resource_calls(&world, "core"), 0);
    assert_eq!(resource_calls(&world, "graphql"), 0);
}

#[test]
fn permission_auth_and_network_errors_do_not_authorize_switching_api() {
    for said in [
        "gh: Resource not accessible by integration (HTTP 403)",
        "gh: Bad credentials (HTTP 401)",
        "dial tcp: network is unreachable",
    ] {
        let world = World::new();
        world.issue(146, &["ready-for-agent"]);
        world.with_gh(|gh| gh["quota_fail"] = json!({"graphql":said}));
        let error = world.afk(&["plan"]).error();
        assert!(error.contains(said), "{error}");
        assert!(!error.contains("fallback"), "{error}");
        assert!(
            world.gh_calls().iter().all(|c| !c.iter().any(|a| a.contains("/pulls?state=open")))
        );
    }
}

#[test]
fn rest_fallback_keeps_blockers_past_page_one_and_fleet_locks() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh| {
        for number in 200..301 {
            gh["issues"].as_array_mut().unwrap().push(json!({"number":number,"title":"dependency","labels":[],"state":if number==300{"open"}else{"closed"}}));
        }
        gh["edges"]=json!({"146":(200..301).collect::<Vec<_>>()});
    });
    primary(&world, "graphql");
    let planned = plan(&world);
    assert_eq!(planned["frontier"], json!([]));
    assert!(planned["excluded"].as_array().unwrap().iter().any(|r|r["number"]==146&&r["reason"].as_str().is_some_and(|s|s.contains("#300"))),"{planned}");
    world.close(300, "completed");
    assert_eq!(plan(&world)["frontier"][0]["number"], 146);
    world.issue(500, &["fleet-lock"]);
    assert_eq!(plan(&world)["frontier"], json!([]));
}

#[test]
fn rest_exhaustion_keeps_a_lock_and_late_blocker_in_graphql() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh| {
        for number in 200..301 {
            gh["issues"].as_array_mut().unwrap().push(json!({"number":number,"title":"dependency","labels":[],"state":if number==300{"open"}else{"closed"}}));
        }
        gh["edges"]=json!({"146":(200..301).collect::<Vec<_>>()});
        gh["issues"][0]["blockers_more"]=json!(true);
        gh["issues"][0]["blockers_at_rebuild"]=json!([]);
    });
    primary(&world, "core");
    assert_eq!(plan(&world)["frontier"], json!([]));
    world.close(300, "completed");
    assert_eq!(plan(&world)["frontier"][0]["number"], 146);
    world.issue(500, &["fleet-lock"]);
    assert_eq!(plan(&world)["frontier"], json!([]));
    let doctor = world.afk(&["doctor", "--now", "1000"]).json;
    assert_eq!(
        doctor["fleet"]["github_api"]["for_rest_reads"], "graphql when equivalent",
        "{doctor}"
    );
}

#[test]
fn quota_notes_are_isolated_by_authenticated_identity_and_host() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    primary(&world, "graphql");
    plan(&world);
    world.gh_calls();
    world.with_gh(|gh| {
        gh["quota_fail"] = json!({});
        gh["login"] = json!("second-user");
    });
    plan(&world);
    assert!(resource_calls(&world, "graphql") > 0);
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    let out = command.env("GH_HOST", "github.example.test").args(["plan"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let dirs = std::fs::read_dir(world.cache().join("afkconvoy/github-quota")).unwrap().count();
    assert_eq!(dirs, 2);
}

#[test]
fn confirmed_rest_rejection_can_post_one_graphql_verdict_comment() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.afk(&["claim", "--issue", "146", "--instance", "alpha", "--now", "1000"]).ok();
    primary(&world, "core");
    world
        .afk(&[
            "verdict",
            "pass",
            "--issue",
            "146",
            "--stage",
            "build",
            "--text",
            "built \"once\"\nwith a newline",
            "--role-commit",
            "r1",
            "--team-commit",
            "t1",
        ])
        .ok();
    let gh = world.gh();
    assert_eq!(gh["comments"]["146"].as_array().unwrap().len(), 1, "{gh}");
}

#[test]
fn losing_a_mutation_receipt_does_not_post_a_second_comment() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.afk(&["claim", "--issue", "146", "--instance", "alpha", "--now", "1000"]).ok();
    primary(&world, "core");
    world.with_gh(|gh| gh["write_receipt_lost"] = json!(true));
    let error = world
        .afk(&[
            "verdict",
            "pass",
            "--issue",
            "146",
            "--stage",
            "build",
            "--text",
            "built",
            "--role-commit",
            "r1",
            "--team-commit",
            "t1",
        ])
        .error();
    assert!(error.contains("write outcome uncertain"), "{error}");
    let gh = world.gh();
    assert_eq!(gh["comments"]["146"].as_array().unwrap().len(), 1);
    let writes = world
        .gh_calls()
        .iter()
        .filter(|c| c.iter().any(|s| s.contains("ConvoyQuotaWrite")))
        .count();
    assert_eq!(writes, 1);
}

#[test]
fn ordinary_turn_with_rest_exhausted_still_gates_lands_and_closes_its_issue() {
    let world = World::new();
    world.set_gate("true");
    world.afk(&["gate"]).ok();
    let worktree = world.worker(146, "quota.txt", "land through the turn");
    primary(&world, "core");
    let result = world.afk_in(&worktree, &["land", "--issue", "146"]).ok();
    assert_eq!(result["outcome"], "merged", "{result}");
    assert_eq!(world.state(146), "closed");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), result["commit"].as_str().unwrap());
    let tree = crate::common::git(
        &worktree,
        &["rev-parse", &format!("{}^{{tree}}", result["commit"].as_str().unwrap())],
    );
    assert!(
        !world
            .remote(&["for-each-ref", "--format=%(refname)", &format!("refs/convoy/gate/{tree}-*")])
            .is_empty()
    );
}

#[test]
fn ordinary_turn_waits_for_native_relationships_before_running_its_gate() {
    let world = World::new();
    let marker = world.root.join("gate-ran");
    world.set_gate(&format!("touch '{}'", marker.display()));
    world.afk(&["gate"]).ok();
    std::fs::remove_file(&marker).unwrap();
    let worktree = world.worker(146, "quota.txt", "wait for native facts");
    primary(&world, "graphql");
    let error = world.afk_in(&worktree, &["land", "--issue", "146"]).error();
    assert!(error.contains("native/manual") && error.contains("reset"), "{error}");
    assert!(!marker.exists());
    assert_eq!(world.state(146), "open");
}

#[test]
fn a_native_cross_tracker_relationship_past_page_one_keeps_the_issue_off() {
    let world = World::new();
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, "acme/tracker");
    world.with_tracker(|gh|gh["issues"].as_array_mut().unwrap().push(json!({"number":146,"title":"tracked","labels":["ready-for-agent","repo:acme/dev"],"state":"open"})));
    world.pr(70, 999, "work");
    world.with_gh(|gh|{
        let links:Vec<_>=(1000..1100).map(|number|json!({"number":number,"repository":{"name":"other","owner":{"login":"acme"}}})).chain(std::iter::once(json!({"number":146,"repository":{"name":"tracker","owner":{"login":"acme"}}}))).collect();
        gh["prs"][0]["body"]=json!("No body association.");gh["prs"][0]["linked"]=json!(links);
    });
    // Only REST is exhausted: the complete native GraphQL connection stands.
    primary(&world, "core");
    let planned = plan(&world);
    assert_eq!(planned["frontier"], json!([]), "{planned}");
    assert!(
        planned["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["number"] == 146 && r["reason"] == "has an open linked PR"),
        "{planned}"
    );
    let calls = world.gh_calls();
    assert!(
        calls.iter().any(|c| c.iter().any(|s| s == "endCursor=100")),
        "native connection must reach its second page"
    );
}

#[test]
fn comments_past_page_one_keep_the_latest_stage_record_on_both_backends() {
    for api in ["core", "graphql"] {
        let world = crate::rebuild::bound();
        world.issue(146, &["flow:feature"]);
        world.afk(&["claim", "--issue", "146", "--instance", "alpha", "--now", "1000"]).ok();
        for _ in 0..101 {
            world.comment(146, "older discussion");
        }
        world
            .afk(&[
                "verdict",
                "pass",
                "--issue",
                "146",
                "--stage",
                "build",
                "--text",
                "built",
                "--role-commit",
                "r1",
                "--team-commit",
                "t1",
            ])
            .ok();
        world.afk(&["release", "--issue", "146", "--instance", "alpha"]).ok();
        primary(&world, api);
        let planned = plan(&world);
        assert_eq!(planned["frontier"][0]["number"], 146, "{planned}");
        assert_eq!(planned["frontier"][0]["stage"], "review", "{planned}");
    }
}

#[test]
fn a_native_pr_beyond_the_list_limit_is_read_and_never_treated_as_no_open_prs() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh|{
        gh["prs"]=json!((0..1001).map(|n|json!({"number":n+1000,"title":"PR","body":"No closing keyword.","state":"open","fork":n<1000,
            "headRefName":"unrelated","headRefOid":"0000000000000000000000000000000000000000","linked":if n==1000{json!([146])}else{json!([])}})).collect::<Vec<_>>());
    });
    let planned = plan(&world);
    assert_eq!(planned["frontier"], json!([]), "{planned}");
    assert!(
        planned["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["number"] == 146 && r["reason"] == "has an open linked PR")
    );
    primary(&world, "graphql");
    let error = world.afk(&["plan"]).error();
    assert!(error.contains("native/manual"), "{error}");
}

#[test]
fn response_headers_supply_reset_and_secondary_http_date_backoff() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh|{
        gh["quota"]=json!({});
        gh["quota_fail"]=json!({"graphql":{"stderr":"GraphQL: API rate limit already exceeded","stdout":"HTTP/2.0 403 Forbidden\nX-RateLimit-Remaining: 0\nX-RateLimit-Reset: 4000000123\nX-RateLimit-Resource: graphql\n\n{}"}});
    });
    assert_eq!(plan(&world)["frontier"][0]["number"], 146);
    let status = world.afk(&["doctor", "--now", "1000"]).json;
    assert_eq!(status["fleet"]["github_api"]["graphql"]["reset"], 4_000_000_123u64, "{status}");

    let limited = World::new();
    limited.issue(146, &["ready-for-agent"]);
    limited.with_gh(|gh|gh["quota_fail"]=json!({"graphql":{"stderr":"gh: secondary rate limit","stdout":"HTTP/2.0 403 Forbidden\nRetry-After: Fri, 01 Jan 2038 00:00:00 GMT\n\n{}"}}));
    let error = limited.afk(&["plan"]).error();
    assert!(error.contains("secondary rate limit"), "{error}");
    let file = quota_files(&limited)
        .into_iter()
        .find(|p| p.file_name().unwrap() == "secondary.json")
        .unwrap();
    let note: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(note["reset"], afkconvoy::doctor::watching::since("2038-01-01T00:00:00Z").unwrap());
}

#[test]
fn watching_keeps_all_pr_endings_and_late_intake_records_in_both_directions() {
    for api in ["core", "graphql"] {
        let world = World::new();
        world.list_checkout(&world.clone);
        world.issue(3, &[]);
        world.with_gh(|gh|{
            let mut comments:Vec<_>=(0..101).map(|n|json!({"id":n+1,"at":900+n,"body":"discussion","html_url":"comment"})).collect();
            comments[100]["body"]=json!("<!--convoy:intake workflow=feature by=train-->");gh["comments"]["3"]=json!(comments);
            gh["prs"]=json!((0..101).map(|n|json!({"number":200+n,"title":"ended","body":"","headRefName":"work","headRefOid":"","state":"closed","closed_at":1000,"updated_at":1000})).collect::<Vec<_>>());
        });
        primary(&world, api);
        let answer = world
            .afk(&["doctor", "--all", "--host", "h1", "--since", "1000", "--now", "1100"])
            .ok();
        let watching = &answer["checkouts"][0]["doctor"]["watching"];
        assert_eq!(watching["ended"].as_array().unwrap().len(), 102, "{answer}");
        assert_eq!(answer["until"], 1100, "{answer}");
    }
}

fn with_token(world: &World, token: &str) -> crate::common::Answer {
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("GH_TOKEN", token)
        .args(["plan", "--instance", "alpha", "--now", "1000"])
        .output()
        .unwrap();
    crate::common::Answer {
        json: serde_json::from_slice(&out.stdout).unwrap(),
        code: out.status.code().unwrap_or(1),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn overriding_credentials_resolve_identity_without_exhausted_status_probes() {
    for api in ["core", "graphql"] {
        let world = World::new();
        world.issue(146, &["ready-for-agent"]);
        world.with_gh(|gh| {
            gh["identity_by_token"] = json!({"fixture-token-a":"user-a","fixture-token-b":"user-b"})
        });
        primary(&world, api);
        assert_eq!(with_token(&world, "fixture-token-a").ok()["frontier"][0]["number"], 146);
        world.gh_calls();
        assert_eq!(with_token(&world, "fixture-token-a").ok()["frontier"][0]["number"], 146);
        assert_eq!(
            resource_calls(&world, api),
            0,
            "a known credential must not rediscover its identity via the exhausted API"
        );
        assert_eq!(with_token(&world, "fixture-token-b").ok()["frontier"][0]["number"], 146);
        assert!(
            resource_calls(&world, api) > 0,
            "a changed credential must discover its own login/quota"
        );
        for file in quota_files(&world) {
            let text = std::fs::read_to_string(file).unwrap();
            assert!(
                !text.contains("fixture-token-a") && !text.contains("fixture-token-b"),
                "credentials must not be stored"
            );
        }
    }
}

#[test]
fn an_unidentified_credential_with_both_quotas_zero_waits_without_repeated_probes() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.with_gh(|gh|gh["quota"]=json!({"resources":{
        "core":{"remaining":0,"reset":4_000_000_000u64},"graphql":{"remaining":0,"reset":4_000_000_100u64}}}));
    let error = with_token(&world, "fixture-token").error();
    assert!(
        error.contains("both GitHub primary quotas exhausted") && error.contains("reset"),
        "{error}"
    );
    world.gh_calls();
    let error = with_token(&world, "fixture-token").error();
    assert!(error.contains("identity waits"), "{error}");
    let calls: Vec<Vec<String>> = serde_json::from_value(world.gh()["calls"].clone()).unwrap();
    assert!(
        calls.iter().all(|c| c.first().is_none_or(|s| s != "api")
            && !c.starts_with(&["pr".into(), "list".into()])),
        "{calls:?}"
    );
}

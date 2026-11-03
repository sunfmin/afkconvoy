use crate::common;

use std::path::PathBuf;

use afkconvoy_core::lease::LEASE;
use common::{World, commit, git};
use serde_json::{Value, json};

/// A moment past the lease, for a heartbeat stamped at 1000.
const LATER: u64 = 1000 + LEASE + 1;

fn tick_as(world: &World, instance: &str, host: &str, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", instance, "--host", host, "--now", &now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// `instance` on `host` claims `issue` at `now`, with nothing else done for it.
fn claimed(world: &World, issue: u64, instance: &str, host: &str, now: u64) {
    let (issue, now) = (issue.to_string(), now.to_string());
    let args = ["claim", "--issue", &issue, "--instance", instance, "--host", host, "--now", &now];
    assert_eq!(world.afk(&args).ok()["won"], true);
}

fn holder(world: &World, issue: u64) -> String {
    let claims = world.claims();
    claims.iter().find(|claim| claim.0 == issue).map(|claim| claim.1.clone()).unwrap_or_default()
}

fn takeover(world: &World, more: &[&str]) -> common::Answer {
    let mut args = vec!["takeover"];
    args.extend(more);
    world.afk(&args)
}

/// The worker always names its issue, including in an unbound repo.
fn worker_command(world: &World, args: &[&str]) -> common::Answer {
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("AFKCONVOY_ISSUE", "7")
        .args(args)
        .output()
        .unwrap();
    common::Answer {
        json: serde_json::from_slice(&out.stdout).unwrap(),
        code: out.status.code().unwrap(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn a_taken_workers_verdict_land_and_every_gate_refuse_without_writing() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    claimed(&world, 7, "alpha", "h1", 1000);
    takeover(&world, &["--from", "alpha", "--instance", "beta", "--host", "h2", "--now", "2860"])
        .ok();
    let refs = world.remote(&["show-ref"]);
    let comments = world.comments(7);
    world.watch_pushes(None);
    for args in [
        vec![
            "verdict",
            "pass",
            "--issue",
            "7",
            "--stage",
            "build",
            "--text",
            "done",
            "--role-commit",
            "r1",
            "--team-commit",
            "t1",
        ],
        vec!["land", "--issue", "7"],
        vec!["gate"],
        vec!["gate", "--fast"],
        vec!["gate", "--slow"],
        vec!["gate", "--land"],
        vec!["gate", "--host", "h2"],
    ] {
        assert_eq!(
            worker_command(&world, &args).error(),
            "the claim on #7 is held by instance beta on host h2 since 2860: this worker's work was taken; stop",
            "{args:?}"
        );
    }
    assert_eq!(world.remote(&["show-ref"]), refs, "no candidate or gate record");
    assert_eq!(world.comments(7), comments, "no stage record");
    assert!(world.pushes().is_empty());
    assert!(!world.clone.join(".git/convoy-turn.json").exists(), "no local turn receipt");
    assert!(!world.root.join("executed").exists(), "the gate did not start");
}

#[test]
fn a_new_instance_on_the_same_host_keeps_its_workers_commands() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick_as(&world, "alpha", "h1", 1000);
    let terminal = world.orca()["terminals"][0]["handle"].clone();
    takeover(
        &world,
        &["--from", "alpha", "--instance", "beta", "--host", "h1", "--now", "1060", "--yes"],
    )
    .ok();
    assert_eq!(tick_as(&world, "beta", "h1", 1060)["errors"], json!([]));
    let terminals = world.orca()["terminals"].as_array().unwrap().clone();
    let open: Vec<_> = terminals.iter().filter(|t| t["open"] == true).collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["handle"], terminal, "the same worker keeps its terminal");
    let declared = worker_command(
        &world,
        &[
            "verdict",
            "pass",
            "--issue",
            "7",
            "--stage",
            "build",
            "--text",
            "done",
            "--role-commit",
            "r1",
            "--team-commit",
            "t1",
        ],
    )
    .ok();
    assert_eq!(declared["declared"], true);
    assert!(worker_command(&world, &["land", "--issue", "7"]).error().contains("no open PR"));
    world.set_gate("true");
    assert_eq!(worker_command(&world, &["gate"]).ok()["status"], "green");
}

#[test]
fn a_gate_started_before_a_takeover_cannot_record_after_it() {
    let world = World::new();
    world.set_gate(
        "afkconvoy takeover --from alpha --instance beta --host h2 --now 2860 >/dev/null",
    );
    world.issue(7, &["ready-for-agent"]);
    claimed(&world, 7, "alpha", "h1", 1000);
    let error = worker_command(&world, &["gate"]).error();
    assert!(error.contains("this worker's work was taken; stop"), "{error}");
    assert_eq!(holder(&world, 7), "beta");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/gate/"]).is_empty());
}

#[test]
fn a_failed_close_of_a_taken_worker_is_retried_without_losing_its_worktree() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick_as(&world, "alpha", "h1", 1000);
    let path = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    takeover(&world, &["--from", "alpha", "--instance", "beta", "--host", "h2", "--now", "2860"])
        .ok();
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    let failed = tick_as(&world, "alpha", "h1", 2860);
    assert_eq!(failed["errors"][0]["do"], "stop_taken", "{failed}");
    assert!(path.is_dir());
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(false));
    assert_eq!(tick_as(&world, "alpha", "h1", 2950)["errors"], json!([]));
    assert!(
        world.orca()["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["worktreePath"] == path.to_str().unwrap())
            .all(|t| t["open"] != true)
    );
    assert!(path.is_dir());
}

#[test]
fn a_slept_hosts_next_tick_stops_its_taken_worker_and_keeps_unpushed_work() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick_as(&world, "alpha", "h1", 1000);
    let path = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    let head = commit(&path, "only-here.txt", "work while the host slept\n");
    world.watch_pushes(None);
    takeover(&world, &["--from", "alpha", "--instance", "beta", "--host", "h2", "--now", "2860"])
        .ok();
    // Both hosts now read the clock after the sleep; beta's new heartbeat
    // is live, so alpha must close its worker and leave beta's claim alone.
    assert_eq!(tick_as(&world, "alpha", "h1", 2860)["errors"], json!([]));
    assert_eq!(holder(&world, 7), "beta");
    assert!(path.is_dir());
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), head);
    let orca = world.orca();
    let workers: Vec<_> = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["worktreePath"] == path.to_str().unwrap() && !t["command"].is_null())
        .collect();
    assert!(!workers.is_empty());
    assert!(workers.iter().all(|t| t["open"] != true));
    assert!(!world.pushes().iter().any(|name| name.starts_with("refs/heads/")));
    assert!(!world.remote(&["rev-list", "--all"]).lines().any(|sha| sha == head));
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "2860"]).ok();
    let finding = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "taken_worktree")
        .expect("doctor names the retained local commit");
    assert!(finding["what"].as_str().unwrap().contains("1 commit the remote does not have"));
    assert!(finding["evidence"].as_str().unwrap().contains(path.to_str().unwrap()));
    // The fix names no push; the checkout path it names may well say one.
    let fix = finding["fix"].as_str().unwrap().replace(path.to_str().unwrap(), "");
    assert!(!fix.contains("push"), "{fix}");
    assert_eq!(tick_as(&world, "alpha", "h1", 2950)["errors"], json!([]));
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), head, "later ticks retain the work too");
}

#[test]
fn a_tick_reclaims_stale_claims_into_its_free_slots_ahead_of_the_frontier() {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
    for issue in [2, 3, 4, 5, 6, 8] {
        world.issue(issue, &["ready-for-agent"]);
    }
    for issue in [6, 3, 4, 8] {
        claimed(&world, issue, "dead", "h2", 1000);
    }
    world.close(8, "completed");
    claimed(&world, 5, "live", "h3", LATER - 60);
    world.watch_pushes(None);

    let ticked = tick_as(&world, "alpha", "h1", LATER);
    assert_eq!(
        did(&ticked),
        ["unlock 8 done", "reclaim 3 done", "reclaim 4 done", "heartbeat done"],
        "lowest first, and nothing off the frontier while stale work waits"
    );
    assert_eq!(ticked["held"], 2);
    assert_eq!(
        world.claims(),
        [
            (3, "alpha".to_string()),
            (4, "alpha".to_string()),
            (5, "live".to_string()),
            (6, "dead".to_string()),
        ],
        "#6, past the free slots, is left stale; the lock on closed #8 is gone"
    );
    // each take is one push of the claim ref; the heartbeat goes ahead of the first
    assert_eq!(
        world.pushes(),
        [
            "refs/convoy/claim/8",
            "refs/convoy/heartbeat/alpha",
            "refs/convoy/claim/3",
            "refs/convoy/claim/4"
        ]
    );
    // a claim taken is continued: each has a worker in a worktree
    let orca = world.orca();
    let linked: Vec<u64> = orca["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["linkedIssue"].as_u64())
        .collect();
    assert_eq!(linked, [3, 4]);

    // #6 is taken once a slot is free
    world.close(3, "completed");
    let next = did(&tick_as(&world, "alpha", "h1", LATER + 90));
    assert!(next.contains(&"reclaim 6 done".to_string()), "{next:?}");
    assert!(!next.iter().any(|step| step.starts_with("dispatch")), "{next:?}");
}

#[test]
fn a_live_peers_claim_on_another_host_is_never_touched_unattended() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    claimed(&world, 3, "peer", "h2", 1000);
    claimed(&world, 8, "peer", "h2", 1000);
    world.close(8, "completed");
    // a heartbeat exactly the lease old is live to everyone
    for now in [1090, 1000 + LEASE] {
        let ticked = tick_as(&world, "alpha", "h1", now);
        assert_eq!(did(&ticked), [] as [String; 0], "at {now}");
        assert_eq!(world.claims(), [(3, "peer".to_string()), (8, "peer".to_string())]);
    }
    // nor at a start on another host
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1"];
    let (lines, _) = world
        .afk_lines(&[&args[..], &["--second-ms", "0", "--ticks", "1", "--now", "1090"]].concat());
    assert!(!lines.iter().any(|line| line.starts_with("start:")), "{lines:?}");
    assert_eq!(holder(&world, 3), "peer");
}

#[test]
fn a_reclaimed_claim_is_continued_from_what_its_worker_pushed() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    assert_eq!(did(&tick_as(&world, "dead", "h2", 1000))[0], "dispatch 7 done");
    let old = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    let pushed = commit(&old, "work.txt", "x\n");
    git(&old, &["push", "-q", "origin", "HEAD"]);
    // that host is gone, and its worktree with it
    world.with_orca(|orca| {
        orca["worktrees"] = json!([]);
        orca["terminals"] = json!([]);
    });

    let ticked = tick_as(&world, "alpha", "h1", LATER);
    assert_eq!(did(&ticked), ["reclaim 7 done", "heartbeat done"]);
    assert_eq!(holder(&world, 7), "alpha");
    let new = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    assert_eq!(git(&new, &["rev-parse", "HEAD"]), pushed);
    let git_dir = PathBuf::from(git(&new, &["rev-parse", "--absolute-git-dir"]));
    let brief = std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap();
    assert!(brief.starts_with("You are an afkconvoy worker **continuing**"));
}

#[test]
fn a_start_takes_every_claim_stamped_with_its_host_whatever_the_heartbeat() {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
    for issue in [2, 3, 5, 7, 9] {
        world.issue(issue, &["ready-for-agent"]);
    }
    // the fleet that ran here a minute ago, and a peer elsewhere
    for issue in [7, 3, 9] {
        claimed(&world, issue, "before", "h1", 1000);
    }
    claimed(&world, 5, "peer", "h2", 1000);

    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1"];
    let (lines, _) = world
        .afk_lines(&[&args[..], &["--second-ms", "0", "--ticks", "2", "--now", "1060"]].concat());
    assert_eq!(
        lines[0],
        "start: took #3, #7, #9 stamped with this host · swept 1 dead heartbeat(s) that held \
         nothing"
    );
    // over its bound of two, it continues what it took and takes nothing new
    assert_eq!(
        lines[1],
        "tick 1: continued #3, #7, #9 · holding 2/2 · 1 past the bound · frontier 1 · next in 90s"
    );
    assert!(!lines[2].contains("dispatched"), "{}", lines[2]);
    assert!(!lines.iter().skip(1).any(|line| line.starts_with("start:")), "taken once: {lines:?}");
    assert_eq!(
        world.claims(),
        [
            (3, "alpha".to_string()),
            (5, "peer".to_string()),
            (7, "alpha".to_string()),
            (9, "alpha".to_string()),
        ]
    );
}

#[test]
fn takeover_lists_the_instances_the_remote_remembers() {
    let world = World::new();
    claimed(&world, 9, "dead", "h2", 1000);
    claimed(&world, 3, "dead", "h2", 1000);
    claimed(&world, 4, "alpha", "h1", LATER - 30);
    world
        .afk(&[
            "heartbeat",
            "--instance",
            "idle",
            "--host",
            "h3",
            "--now",
            &(LATER - 10).to_string(),
        ])
        .ok();
    world.watch_pushes(None);

    let now = LATER.to_string();
    let listed = takeover(&world, &["--list", "--instance", "alpha", "--now", &now]).ok();
    assert_eq!((listed["me"].clone(), listed["lease"].clone()), (json!("alpha"), json!(LEASE)));
    let rows = listed["instances"].as_array().unwrap();
    assert_eq!(
        rows[0],
        json!({"instance": "dead", "host": "h2", "claims": [3, 9], "runs": [], "claim_count": 2,
            "heartbeat_ts": 1000, "heartbeat_age": LEASE + 1, "fresh": false, "is_me": false})
    );
    assert_eq!(rows[1]["instance"], "alpha");
    assert_eq!((rows[1]["fresh"].clone(), rows[1]["is_me"].clone()), (json!(true), json!(true)));
    assert_eq!(rows[2]["instance"], "idle");
    assert_eq!(
        (rows[2]["claim_count"].clone(), rows[2]["heartbeat_age"].clone()),
        (json!(0), json!(10))
    );
    assert_eq!(world.pushes(), [] as [String; 0], "a listing takes nothing");
    // it needs no instance of its own
    assert_eq!(takeover(&world, &["--list", "--now", &now]).ok()["me"], Value::Null);
}

#[test]
fn takeover_from_takes_without_the_lease_and_asks_twice_of_a_fleet_that_looks_alive() {
    let world = World::new();
    claimed(&world, 9, "peer", "h2", 1000);
    claimed(&world, 3, "peer", "h2", 1000);
    claimed(&world, 4, "other", "h3", 1000);
    let from = ["--from", "peer", "--instance", "alpha", "--host", "h1", "--now", "1060"];

    let asked = takeover(&world, &from).ok();
    assert_eq!(asked["action"], "confirm");
    assert_eq!(asked["heartbeat_age"], 60);
    assert!(asked["detail"].as_str().unwrap().contains("--yes"));
    assert_eq!(holder(&world, 3), "peer", "nothing is taken until then");

    let taken = takeover(&world, &[&from[..], &["--yes"]].concat()).ok();
    assert_eq!(taken["action"], "taken");
    assert_eq!((taken["taken"].clone(), taken["lost"].clone()), (json!([3, 9]), json!([])));
    assert_eq!(
        world.claims(),
        [(3, "alpha".to_string()), (4, "other".to_string()), (9, "alpha".to_string())]
    );
    let scan = world.afk(&["scan"]).ok();
    assert_eq!(scan["claims"][0]["host"], "h1", "re-stamped with the taker's host");

    // run again: nothing of that instance is left
    let again = takeover(&world, &from).ok();
    assert_eq!(again["action"], "none");
    // a fleet whose lease lapsed needs no second word
    let later = (1000 + LEASE + 1).to_string();
    let lapsed = ["--from", "other", "--instance", "alpha", "--now", &later];
    assert_eq!(takeover(&world, &lapsed).ok()["taken"], json!([4]));

    let mine = takeover(&world, &["--from", "alpha", "--instance", "alpha", "--now", "1060"]);
    assert!(mine.error().contains("already its own"));
    let error = takeover(&world, &["--from", "peer"]).error();
    assert!(error.contains("--instance"), "{error}");
    assert!(takeover(&world, &[]).error().contains("--list"));
}

#[test]
fn of_two_takers_one_wins_each_claim() {
    let world = World::new();
    claimed(&world, 3, "dead", "h2", 1000);
    claimed(&world, 9, "dead", "h2", 1000);
    let later = LATER.to_string();
    let answers: Vec<Value> = std::thread::scope(|scope| {
        let racers: Vec<_> = (0..5)
            .map(|i| {
                let (world, later) = (&world, &later);
                scope.spawn(move || {
                    let me = format!("taker-{i}");
                    takeover(world, &["--from", "dead", "--instance", &me, "--now", later]).ok()
                })
            })
            .collect();
        racers.into_iter().map(|racer| racer.join().unwrap()).collect()
    });
    for issue in [3, 9] {
        let winners: Vec<&Value> = answers
            .iter()
            .filter(|answer| answer["taken"].as_array().unwrap().contains(&json!(issue)))
            .collect();
        assert_eq!(winners.len(), 1, "#{issue}: {answers:?}");
        assert_eq!(json!(holder(&world, issue)), winners[0]["as"]);
    }
}

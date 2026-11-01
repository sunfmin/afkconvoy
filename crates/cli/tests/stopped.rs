use crate::common;

use std::path::PathBuf;

use common::{World, commit, git};
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
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world with a worker dispatched on issue 7 at `now`.
fn fleet(now: u64) -> World {
    let world = World::new();
    // Every hand-off escalates for the watch: what a worker
    // left is what is tried here.
    world.set_gate_config("gate_command: ./gate.sh\ngate_seconds: 600\nescalate_to: human");
    world.issue(7, &["ready-for-agent", "bug"]);
    assert_eq!(did(&tick(&world, now)), ["dispatch 7 done", "heartbeat done"]);
    world
}

fn worktree(world: &World) -> PathBuf {
    PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap())
}

fn declare(world: &World, phase: &str) -> String {
    world.comment(7, &format!("<!--convoy:verdict phase={phase}-->\nwhat the worker says"))
}

/// How many escalation comments issue 7 carries.
fn escalations(world: &World) -> usize {
    world.comments(7).iter().filter(|body| body.contains("convoy:escalation")).count()
}

/// The escalation comment. A work account is written after it and is not this.
fn escalation_comment(world: &World) -> String {
    world
        .comments(7)
        .into_iter()
        .find(|body| body.starts_with("<!--convoy:escalation"))
        .expect("an escalation comment")
}

fn act(world: &World, command: &str, instance: &str, more: &[&str]) -> common::Answer {
    let mut args = vec![command, "--issue", "7", "--instance", instance, "--now", "1100"];
    args.extend(more);
    world.afk(&args)
}

/// The worker's terminal reports `state` since `since`, its last output at `output` (epoch seconds).
fn report(world: &World, state: &str, since: u64, output: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["command"] == common::trusted("agent") {
                terminal["agent"] = json!({"state": state, "stateStartedAt": since * 1000});
                terminal["lastOutputAt"] = json!(output * 1000);
            }
        }
    });
}

#[test]
fn a_busy_worker_is_left_with_no_github_read() {
    let world = fleet(1000);
    declare(&world, "escalate");
    report(&world, "working", 1000, 1080);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1090)), ["judge 7 nothing", "heartbeat done"]);
    let calls: Vec<String> = world.gh_calls().iter().map(|call| call.join(" ")).collect();
    assert!(!calls.iter().any(|call| call.contains("issues/7/comments")), "{calls:?}");

    // an open session turn stays busy past the grace period
    report(&world, "working", 1000, 1080);
    assert_eq!(did(&tick(&world, 1080 + 300))[0], "judge 7 nothing");
    assert_eq!(world.claims().len(), 1);

    // the session says the turn ended, and the grace has passed
    report(&world, "done", 1000, 1000);
    assert_eq!(did(&tick(&world, 1000 + 300))[0], "judge 7 done");
    assert_eq!(world.claims(), []);
}

#[test]
fn an_orca_that_cannot_be_asked_is_an_error_never_a_worker_that_is_gone() {
    let world = fleet(1000);
    declare(&world, "escalate");
    world.with_orca(|orca| orca["ps_truncated"] = json!(true));
    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert!(ticked["errors"][0]["error"].as_str().unwrap().contains("cut short"));
    assert_eq!(world.claims().len(), 1);

    world.with_orca(|orca| {
        orca["ps_truncated"] = json!(false);
        orca["ps_fails"] = json!(true);
    });
    let ticked = tick(&world, 1180);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert_eq!(world.claims().len(), 1);
    assert_eq!(world.state(7), "open");
}

#[test]
fn a_recent_sign_of_life_is_left_whatever_was_declared() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let world = fleet(now);
    declare(&world, "escalate");
    // the worktree was touched moments ago
    assert_eq!(did(&tick(&world, now + 90))[0], "judge 7 nothing");
    assert_eq!(did(&tick(&world, now + 400))[0], "judge 7 done");
}

#[test]
fn already_satisfied_with_nothing_on_the_branch_closes_the_issue() {
    let world = fleet(1000);
    world.say_concurrency(&world.clone, 2);
    world.issue(9, &["ready-for-agent"]);
    world.issue(10, &["ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1090))[1], "dispatch 9 done");
    declare(&world, "escalate");
    declare(&world, "already-satisfied");
    let dir = worktree(&world);
    world.watch_pushes(None);

    // (its worker, silent with no clock to read, was nudged a grace period ago)
    let ticked = tick(&world, 1390);
    assert_eq!(
        did(&ticked),
        ["judge 7 done", "judge 9 nothing", "dispatch 10 done", "heartbeat done"],
        "the latest verdict wins, and the slot it frees is filled"
    );
    assert_eq!(world.state(7), "closed");
    assert!(!dir.exists(), "the worktree is removed");
    assert_eq!(world.claims().iter().map(|c| c.0).collect::<Vec<_>>(), [9, 10]);
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"], "nothing is relabelled");
    let comments = world.comments(7);
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:account ")).count(),
        1,
        "closing leaves one work account: {comments:?}"
    );
}

#[test]
fn already_satisfied_on_a_branch_only_behind_the_base_closes_the_issue() {
    let world = fleet(1000);
    let dir = worktree(&world);
    commit(&world.clone, "landed.txt", "landed while the worker checked\n");
    git(&world.clone, &["push", "-q", "origin", "main"]);
    assert_eq!(git(&dir, &["rev-list", "--count", "origin/main..HEAD"]), "0");
    assert_ne!(git(&dir, &["rev-list", "--count", "HEAD..origin/main"]), "0");
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    declare(&world, "already-satisfied");

    assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert!(!dir.exists());
    assert_eq!(escalations(&world), 0);
}

#[test]
fn already_satisfied_with_commits_ahead_and_an_equal_tree_escalates() {
    for reverted in [false, true] {
        let world = fleet(1000);
        let dir = worktree(&world);
        if reverted {
            commit(&dir, "work.txt", "x\n");
            git(&dir, &["revert", "--no-edit", "HEAD"]);
        } else {
            git(&dir, &["commit", "--allow-empty", "-m", "no content change"]);
        }
        assert_ne!(git(&dir, &["rev-list", "--count", "origin/main..HEAD"]), "0");
        assert_eq!(git(&dir, &["diff", "origin/main", "HEAD", "--"]), "");
        declare(&world, "already-satisfied");

        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.state(7), "open");
        assert_eq!(world.claims(), []);
        assert!(dir.exists(), "commits past the merge base are evidence even with an equal tree");
        assert_eq!(escalations(&world), 1);
        assert!(escalation_comment(&world).contains("commits past the merge base"));
    }
}

#[test]
fn already_satisfied_over_work_on_the_branch_escalates_and_keeps_the_worktree() {
    for dirty in [false, true] {
        let world = fleet(1000);
        let dir = worktree(&world);
        if dirty {
            std::fs::write(dir.join("scratch.txt"), "x").unwrap();
        } else {
            commit(&dir, "work.txt", "x\n");
        }
        commit(&world.clone, "landed.txt", "only on the base branch\n");
        git(&world.clone, &["push", "-q", "origin", "main"]);
        let said = declare(&world, "already-satisfied");

        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.state(7), "open");
        assert_eq!(world.claims(), []);
        assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
        assert!(dir.exists(), "the worktree is evidence");
        assert_eq!(world.open_terminals(), Vec::<String>::new(), "its worker is ended");
        // the branch record, the worker, the verdict, the escalation, the account
        let comments = world.comments(7);
        let escalation = escalation_comment(&world);
        assert!(escalation.starts_with("<!--convoy:escalation claim="), "{escalation}");
        assert!(escalation.contains("escalated to a human"));
        assert!(escalation.contains("its branch holds work") && escalation.contains(&said));
        let file = if dirty { "scratch.txt" } else { "work.txt" };
        assert!(escalation.contains(file), "{escalation}");
        assert!(!escalation.contains("landed.txt"), "{escalation}");
        assert_eq!(
            comments.iter().filter(|body| body.starts_with("<!--convoy:account ")).count(),
            1,
            "{comments:?}"
        );
    }
}

#[test]
fn escalate_and_an_unknown_phase_escalate() {
    for (phase, says) in [
        ("escalate", "declared `escalate`"),
        // what `escalate` was called: comments written under it are still read
        ("needs-human", "declared `escalate`"),
        ("giving-up", "cannot read"),
    ] {
        let world = fleet(1000);
        let dir = worktree(&world);
        let said = declare(&world, phase);
        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.claims(), []);
        assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
        let escalation = escalation_comment(&world);
        assert!(escalation.contains(says) && escalation.contains(&said), "{escalation}");
        assert!(!dir.exists(), "a worktree whose branch holds no work is removed");
        // off the frontier: nothing dispatches it again
        assert_eq!(did(&tick(&world, 1180)), [] as [String; 0]);
    }
}

/// What was typed at the worker of issue 7.
fn typed(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let worker =
        terminals.iter().find(|terminal| terminal["command"] == common::trusted("agent")).unwrap();
    let sent = worker["sent"].as_array().unwrap();
    sent.iter().map(|line| line["text"].as_str().unwrap().to_string()).collect()
}

/// Every `orca terminal read` since the last look.
fn screen_reads(world: &World) -> Vec<Vec<String>> {
    let calls = world.orca_calls();
    calls.into_iter().filter(|call| common::asked(call) == "terminal read").collect()
}

/// How many lines of its screen each read since the last look asked for.
fn read_limits(world: &World) -> Vec<String> {
    let reads = screen_reads(world);
    reads
        .iter()
        .map(|read| {
            let limit = read.iter().position(|arg| arg == "--limit").unwrap();
            read[limit + 1].clone()
        })
        .collect()
}

/// The one since, kept in the worktree's git dir.
fn background_file(world: &World) -> PathBuf {
    let dir = worktree(world);
    PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"])).join("convoy-background")
}

/// The worker-health record as kept. It holds no copy of the since.
fn health_record(world: &World) -> Value {
    let path = world.cache().join("afkconvoy/worker-health.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The worker's terminal shows `screen`.
fn show(world: &World, screen: &[&str]) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!(screen);
        }
    });
}

/// What Claude Code's terminal shows as it waits on a gate it started in the
/// background, with a monitor on it.
const WAITING: [&str; 5] = [
    "⏺ Monitor(gate --land finishing for #7)",
    "✻ Worked for 14s · done 11:03 PM · 1 shell, 1 monitor still running",
    "❯ ",
    "─────",
    "  ⏵⏵ bypass permissions on · 1 shell, 1 monitor · ↓ to manage",
];

/// …and once that work ended.
const ENDED: [&str; 4] = [
    "✻ Worked for 14s · done 11:03 PM · 1 shell, 1 monitor still running",
    "❯ ",
    "─────",
    "  ⏵⏵ bypass permissions on",
];

/// The lines one tick of `run` at `now` prints.
fn run_line(world: &World, now: u64) -> Vec<String> {
    let now = now.to_string();
    let args = ["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    let more = ["--instance", "alpha", "--ticks", "1", "--now", &now];
    world.afk_lines(&[&args[..], &more[..]].concat()).0
}

#[test]
fn a_worker_waiting_on_its_own_background_work_is_not_silent_until_it_ends() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &WAITING);
    world.orca_calls();

    // past grace, but waiting on its gate: neither nudged nor escalated
    let ticked = tick(&world, 1300);
    assert_eq!(did(&ticked), ["judge 7 waiting", "heartbeat done"]);
    assert_eq!(ticked["did"][0]["waiting_since"], 1300, "{ticked}");
    assert_eq!(typed(&world).len(), 1, "only its brief");
    assert!(read_limits(&world).contains(&"12".to_string()), "background activity is observed");
    let ticked = tick(&world, 1800);
    assert_eq!(did(&ticked), ["judge 7 waiting", "heartbeat done"]);
    assert_eq!(ticked["did"][0]["waiting_since"], 1300, "since when it was first seen");
    assert_eq!(world.claims().len(), 1);
    // the tick's line says so, and since when
    let lines = run_line(&world, 1820);
    let said = "waiting on its own background work #7 (since 1970-01-01 00:21 UTC, 8m ago)";
    assert!(lines.iter().any(|line| line.contains(said)), "{lines:?}");

    // Once background work ends, new observations precede any recovery.
    show(&world, &ENDED);
    assert_eq!(did(&tick(&world, 1900)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 2200)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(typed(&world).len(), 1);
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn a_worker_waiting_on_its_own_background_work_has_one_since() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &WAITING);
    world.orca_calls();

    // The tick line, worker health and the stamp name the same moment.
    // One read of the screen answers both the wait and the since.
    let ticked = tick(&world, 1300);
    assert_eq!(ticked["did"][0]["waiting_since"], 1300, "{ticked}");
    assert_eq!(read_limits(&world).iter().filter(|limit| *limit == "12").count(), 1);
    assert_eq!(std::fs::read_to_string(background_file(&world)).unwrap().trim(), "1300");
    assert!(health_record(&world)["workers"][0].get("background").is_none());
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1301"]).json;
    let worker = &doctor["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "waiting for background work");
    assert_eq!(worker["background"], 1300, "{worker}");

    // The next tick reads that stamp. It does not read the screen again, and
    // it does not say the wait began now.
    world.orca_calls();
    let again = tick(&world, 1500);
    assert_eq!(again["did"][0]["waiting_since"], 1300, "{again}");
    assert_eq!(read_limits(&world).iter().filter(|limit| *limit == "12").count(), 0);
    let lines = run_line(&world, 1520);
    let said = "waiting on its own background work #7 (since 1970-01-01 00:21 UTC, 3m ago)";
    assert!(lines.iter().any(|line| line.contains(said)), "{lines:?}");

    // The turn is open again: no since.
    report(&world, "working", 1700, 1700);
    assert_eq!(did(&tick(&world, 1700))[0], "judge 7 nothing");
    assert!(!background_file(&world).exists(), "an open turn keeps no since");
    let open = world.afk(&["doctor", "--host", "h1", "--now", "1701"]).json;
    let worker = &open["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "healthy");
    assert!(worker.get("background").is_none(), "{worker}");

    // A later wait starts from when it is seen again, not from the old stamp.
    report(&world, "done", 1800, 1800);
    show(&world, &WAITING);
    let later = tick(&world, 2200);
    assert_eq!(later["did"][0]["waiting_since"], 2200, "{later}");
    assert_eq!(std::fs::read_to_string(background_file(&world)).unwrap().trim(), "2200");
    let shown = world.afk(&["doctor", "--host", "h1", "--now", "2201"]).json;
    let worker = &shown["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["background"], 2200, "{worker}");
    assert!(health_record(&world)["workers"][0].get("background").is_none());
}

#[test]
fn a_worker_waiting_on_background_work_has_its_next_health_observation_scheduled() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &WAITING);
    tick(&world, 1300);

    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1301"]).json;
    let worker = &doctor["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "waiting for background work");
    assert_eq!(worker["next_check"], 1600, "a fresh observation must not look overdue");
}

#[test]
fn a_worker_still_waiting_on_background_work_is_not_failed_by_a_silence_limit() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &WAITING);
    for now in [1300, 1899, 1900] {
        assert_eq!(did(&tick(&world, now))[0], "judge 7 waiting");
    }
    assert_eq!(typed(&world).len(), 1);
    assert_eq!(world.claims().len(), 1);
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
}

/// Findings whose `what` says a worker has been stopped.
fn stopped_for(answer: &Value) -> Vec<&Value> {
    answer["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["what"].as_str().unwrap().contains("has been stopped for"))
        .collect()
}

#[test]
fn a_worker_idle_past_half_an_hour_on_its_landing_gate_is_not_called_stopped() {
    let world = fleet(1000);
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    report(&world, "done", 1000, 1000);
    show(&world, &WAITING);

    // Past grace, with its PR open: the tick watches, and names the same wait
    // worker health records.
    let ticked = tick(&world, 1300);
    assert_eq!(did(&ticked), ["watch 7 waiting", "heartbeat done"]);
    assert_eq!(ticked["did"][0]["waiting_since"], 1300, "{ticked}");
    let health = world.afk(&["doctor", "--host", "h1", "--now", "1301"]).json;
    let worker = &health["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "waiting for background work");

    // Two hours on, past the gate limit of 600s. The tick still waits — its
    // heartbeat keeps the claim — and doctor raises no stopped finding.
    let later = tick(&world, 8200);
    assert_eq!(did(&later)[0], "watch 7 waiting");
    assert_eq!(later["did"][0]["waiting_since"], 1300, "{later}");
    let said = "waiting on its own background work #7";
    let lines = run_line(&world, 8200);
    assert!(lines.iter().any(|line| line.contains(said)), "{lines:?}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "8200"]).json;
    assert_eq!(stopped_for(&doctor), [] as [&Value; 0], "{doctor}");
    let claims = doctor["claims"].as_array().unwrap();
    let claim = claims.iter().find(|claim| claim["issue"] == 7).expect("claim 7");
    assert_eq!(claim["status"], "landing");
    assert_eq!(claim["worker"]["state"], "stopped");
    assert!(claim["worker"]["quiet"].as_u64().unwrap() >= 1800, "{claim}");

    // The gate ended. The same quiet worker, with nothing of its own running,
    // is stopped, and the fix is a tick.
    show(&world, &ENDED);
    let ended = world.afk(&["doctor", "--host", "h1", "--now", "8200"]).json;
    let stopped = stopped_for(&ended);
    assert_eq!(stopped.len(), 1, "{ended}");
    assert_eq!(
        stopped[0]["fix"],
        format!("afkconvoy tick '{}' --instance alpha", world.clone.display())
    );
}

fn nudge_record(world: &World) -> PathBuf {
    let dir = worktree(world);
    PathBuf::from(common::git(&dir, &["rev-parse", "--absolute-git-dir"])).join("convoy-nudged")
}

#[test]
fn a_nudge_is_never_sent_to_a_terminal_whose_harness_has_exited() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["bare_shell"] = json!(true);
        }
    });
    world.orca_calls();
    let refused = world.afk(&["nudge", "7", "--instance", "alpha", "--now", "1400"]).error();
    assert!(refused.contains("no running worker harness"), "{refused}");
    assert_eq!(typed(&world).len(), 1, "only the brief was sent before the harness exited");
    assert!(!nudge_record(&world).exists(), "a refused nudge is never recorded");
    assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal send"));
}

#[test]
fn a_nudge_that_starts_no_turn_is_not_recorded_and_goes_to_worker_health() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| {
        orca["prompt_stages"] = json!(["input_accepted"]);
        orca["send_screen"] =
            json!(include_str!("screens/kimi-empty-input.txt").lines().collect::<Vec<_>>());
    });
    let refused = world.afk(&["nudge", "7", "--instance", "alpha", "--now", "1400"]).error();
    assert!(refused.contains("started no turn"), "{refused}");
    assert!(!nudge_record(&world).exists());
    world.orca_calls();
    tick(&world, 1490);
    assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal send"));
    let doctor = world.afk(&["doctor", "--now", "1490"]).json;
    assert!(
        doctor["host"]["host_kit"]["worker_health"]["workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|worker| worker["path"] == json!(worktree(&world))),
        "{doctor}"
    );
}

#[test]
fn unclassified_silence_without_a_key_waits_for_the_operator_and_preserves_unpushed_work() {
    let world = fleet(1000);
    let dir = worktree(&world);
    commit(&dir, "work.txt", "x\n");
    world.comment(7, "a comment that is no verdict");
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] = json!(["Ran the tests.", "", "Shall I open the PR? [y/n]", "  "]);
        }
    });
    world.orca_calls();

    assert_eq!(did(&tick(&world, 1300)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 1600)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(world.claims().len(), 1);
    assert_eq!(typed(&world).len(), 1, "no unapproved nudge or second writer");
    assert!(dir.exists());
    let plan = world.afk(&["plan", "--now", "1700"]).ok();
    assert_eq!(plan["host_kit"]["worker_health"]["workers"][0]["action"], "waiting for operator");
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
    assert!(screen_reads(&world).iter().all(|r| r.contains(&"--screen".to_string())));
    assert!(!world.asked_a_harness_about("Shall I open the PR?"));

    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1900"]).json;
    let finding = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["what"].as_str().unwrap().contains("worker health is waiting"))
        .expect("the watch can find the worker");
    assert!(finding["evidence"].as_str().unwrap().contains("no Jev key command"));
    assert!(finding["evidence"].as_str().unwrap().contains("Shall I open the PR?"));
}

#[test]
fn a_verdict_is_never_overridden_by_a_nudge() {
    // declared before any nudge: routed, nothing typed
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    declare(&world, "escalate");
    world.orca_calls();
    assert_eq!(did(&tick(&world, 1300)), ["judge 7 done"]);
    assert_eq!(screen_reads(&world), [] as [Vec<String>; 0], "a worker that declared is not read");
    assert!(!world.comments(7).last().unwrap().contains("nudge"));

    // declared in answer to the nudge: routed as declared, not as silence
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    assert_eq!(did(&tick(&world, 1300))[0], "judge 7 nothing");
    declare(&world, "already-satisfied");
    assert_eq!(did(&tick(&world, 1600)), ["judge 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(typed(&world).len(), 1);
}

#[test]
fn a_manual_nudge_the_worker_did_not_take_is_not_recorded_or_treated_as_an_account_fault() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| orca["send_refused"] = json!(true));
    let args = ["nudge", "7", "--instance", "alpha", "--host", "h1", "--now", "1300"];
    world.afk(&args).error();
    assert!(!nudge_record(&world).exists());
    assert_eq!(world.paused(), json!([]));
    world.with_orca(|orca| orca["send_refused"] = json!(false));
    world.afk(&["nudge", "7", "--instance", "alpha", "--host", "h1", "--now", "1390"]).ok();
    assert_eq!(std::fs::read_to_string(nudge_record(&world)).unwrap(), "1390");
    world.with_orca(|orca| orca["read_fails"] = json!(true));
    tick(&world, 1700);
    assert_eq!(world.claims().len(), 1, "missing observations never authorize escalation");
}

#[test]
fn nudge_is_sent_once_and_only_to_a_silent_worker_of_mine() {
    let nudge = |world: &World, instance: &str, now: &str| {
        world.afk(&["nudge", "--issue", "7", "--instance", instance, "--now", now])
    };
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    let error = nudge(&world, "alpha", "1090").error();
    assert!(error.contains("no stopped worker"), "{error}");
    report(&world, "done", 1000, 1000);
    let error = nudge(&world, "alpha", "1090").error();
    assert!(error.contains("sign of life within the grace period"), "{error}");
    let error = nudge(&world, "beta", "1300").error();
    assert!(error.contains("alpha"), "{error}");
    assert_eq!(typed(&world).len(), 1);

    let answer = nudge(&world, "alpha", "1300").ok();
    assert_eq!(answer["action"], "nudged");
    assert_eq!(typed(&world).len(), 2);
    for now in ["1400", "1700"] {
        let error = nudge(&world, "alpha", now).error();
        assert!(error.contains("nothing was sent"), "{error}");
    }
    assert!(nudge(&world, "alpha", "1700").error().contains("already nudged"));
    assert_eq!(typed(&world).len(), 2);

    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    declare(&world, "escalate");
    let error = nudge(&world, "alpha", "1300").error();
    assert!(error.contains("declared a verdict"), "{error}");
    assert_eq!(typed(&world).len(), 1);
}

#[test]
fn escalate_picks_up_from_wherever_it_was_cut_and_comments_once() {
    let world = fleet(1000);
    let reason = ["--reason", "the schema needs a decision"];
    // cut after the comment: the relabel fails
    world.with_gh(|gh| gh["fail"] = json!(["/labels"]));
    let error = act(&world, "escalate", "alpha", &reason).error();
    assert!(error.contains("injected failure"), "{error}");
    assert_eq!(escalations(&world), 1);
    assert_eq!(world.claims().len(), 1, "the claim is still held");
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);

    // cut after the relabel: the release is refused
    world.with_gh(|gh| gh["fail"] = json!([]));
    world.watch_pushes(Some("claim/7"));
    let error = act(&world, "escalate", "alpha", &reason).error();
    assert!(error.contains("release failed"), "{error}");
    assert_eq!(escalations(&world), 1, "the comment of this claim is there already");
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    assert_eq!(world.claims().len(), 1);

    world.watch_pushes(None);
    let answer = act(&world, "escalate", "alpha", &reason).ok();
    assert_eq!(answer["action"], "escalated");
    assert_eq!(answer["released"], true);
    assert_eq!(answer["worktree"], "removed");
    assert_eq!(world.claims(), []);
    assert_eq!(escalations(&world), 1);
    let escalation = escalation_comment(&world);
    assert!(escalation.contains("the schema needs a decision"), "{escalation}");
    let plan = world.afk(&["plan", "--now", "1700"]).ok();
    let waiting = &plan["waiting_on_human"][0];
    assert_eq!(waiting["issue"], 7);
    assert_eq!(waiting["why"], "escalated: the schema needs a decision");
    assert_eq!(waiting["url"], world.comment_url(7, &escalation));
    assert_eq!(
        waiting["answer"],
        "gh issue edit 7 --add-label ready-for-agent --remove-label ready-for-human"
    );

    // run once more: there is no claim left to end
    world.gh_calls();
    let again = act(&world, "escalate", "alpha", &reason).ok();
    assert!(again["detail"].as_str().unwrap().contains("no claim is held"));
    assert_eq!(escalations(&world), 1);
}

#[test]
fn a_transition_refuses_a_claim_that_is_not_this_instances() {
    let world = fleet(1000);
    declare(&world, "already-satisfied");
    for (command, more) in [("escalate", &["--reason", "x"][..]), ("close", &[][..])] {
        let error = act(&world, command, "beta", more).error();
        assert!(error.contains("alpha"), "{error}");
    }
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert_eq!(world.state(7), "open");
    assert_eq!(escalations(&world), 0);
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
}

#[test]
fn close_is_refused_over_work_and_closes_a_branch_with_none() {
    let world = fleet(1000);
    let dir = worktree(&world);
    std::fs::write(dir.join("scratch.txt"), "x").unwrap();
    let error = act(&world, "close", "alpha", &[]).error();
    assert!(error.contains("its branch holds work"), "{error}");
    assert_eq!(world.state(7), "open");
    assert_eq!(world.claims().len(), 1);

    std::fs::remove_file(dir.join("scratch.txt")).unwrap();
    world.watch_pushes(None);
    let answer = act(&world, "close", "alpha", &[]).ok();
    assert_eq!(answer["action"], "closed");
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert!(!dir.exists());
    assert_eq!(world.pushes(), ["refs/convoy/claim/7"]);
}

/// A command of the worker's run in its worktree as `exclusive`, asked at
/// `asked`, placed in the queue of a host runner held for it (`host_held`)
/// and running there for as long as this lives.
struct Queued {
    _command: common::FixtureProcess,
    _host_runner: common::Running,
}

fn queued(world: &World, asked: u64) -> Queued {
    let host_runner = world.host_held();
    let ran = world.root.join("queued-ran");
    let script = format!("touch '{}'; sleep 600", ran.display().to_string().replace('\'', "'\\''"));
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &worktree(world));
    command
        .env("AFKCONVOY_ISSUE", "7")
        .args(["--now", &asked.to_string(), "exclusive", "--", "sh", "-c", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let command = common::FixtureProcess::spawn(&mut command);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ran.exists() {
        assert!(std::time::Instant::now() < until, "the queued command never ran");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Queued { _command: command, _host_runner: host_runner }
}

#[test]
fn a_worker_waiting_in_the_exclusive_queue_is_busy_until_the_gate_limit() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &["Running the heavy tests alone.", "❯ "]);
    let _queued = queued(&world, 1250);

    // past grace, its screen showing nothing running: it waits in the queue
    let ticked = tick(&world, 1300);
    assert_eq!(did(&ticked), ["judge 7 waiting", "heartbeat done"]);
    assert_eq!(ticked["did"][0]["waiting_since"], 1250, "since it asked: {ticked}");
    assert_eq!(did(&tick(&world, 1250 + 599))[0], "judge 7 waiting");
    assert_eq!(typed(&world).len(), 1, "never nudged for it");

    // A live background command remains a wait past the observation interval.
    let ticked = tick(&world, 1250 + 600);
    assert_eq!(
        did(&ticked)[0],
        "judge 7 waiting",
        "{ticked}; health: {}",
        std::fs::read_to_string(world.cache().join("afkconvoy/worker-health.json")).unwrap()
    );
    assert_eq!(typed(&world).len(), 1, "no nudge");
    assert_eq!(world.claims().len(), 1);
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
    common::fixture_lease_observed();
}

#[test]
fn queued_fixture_reaps_its_workers_and_releases_inherited_descriptors() {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let unrelated = common::FixtureProcess::spawn(
        std::process::Command::new("sleep")
            .arg("600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    );

    // Reproduce a gate's inherited kernel lease, without holding or changing
    // the real host's capacity. The original fixture runs in a child test.
    struct Probe {
        child: Option<std::process::Child>,
        observer: std::fs::File,
        path: tempfile::TempPath,
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            // Also clean up a red reproduction: only this unique descriptor's
            // inheritors are ours, including helpers reparented to pid 1.
            let holders =
                std::process::Command::new("lsof").arg("-t").arg(&self.path).output().unwrap();
            for pid in String::from_utf8_lossy(&holders.stdout).split_whitespace() {
                let pid: u32 = pid.parse().unwrap();
                if pid != std::process::id() {
                    // SAFETY: lsof confirmed this private lease's inheritor.
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                }
            }
            if let Some(child) = &mut self.child {
                let _ = child.wait();
            }
        }
    }

    for (scenario, panics) in [
        ("stopped::a_worker_waiting_in_the_exclusive_queue_is_busy_until_the_gate_limit", false),
        ("doctor::the_host_s_exclusive_queue_is_shown_what_runs_first_then_what_waits", false),
        ("stopped::a_worker_waiting_in_the_exclusive_queue_is_busy_until_the_gate_limit", true),
    ] {
        let rendezvous = tempfile::tempdir().unwrap();
        let tmp = rendezvous
            .path()
            .join("a deep fixture directory with spaces")
            .join("the-tests-run-below-this-directory-to-exercise-long-temporary-paths");
        std::fs::create_dir_all(&tmp).unwrap();
        let (lease, path) = tempfile::NamedTempFile::new().unwrap().into_parts();
        lease.lock_shared().unwrap();
        let observer = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let fd = lease.as_raw_fd();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", scenario, "--nocapture"])
            .env("CONVOY_FIXTURE_LEASE_PROBE", rendezvous.path())
            .env("CONVOY_FIXTURE_LEASE_PANIC", panics.to_string())
            .env("TMPDIR", &tmp)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        // SAFETY: change only this live descriptor's flag in the child.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut probe = Probe { child: Some(command.spawn().unwrap()), observer, path };
        drop(lease);
        assert!(
            matches!(probe.observer.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "a running fixture must retain its inherited lease"
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !rendezvous.path().join("ready").exists() {
            assert!(
                probe.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "{scenario} ended before observation"
            );
            assert!(std::time::Instant::now() < until, "{scenario} never reached observation");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let holders =
            std::process::Command::new("lsof").arg("-Fpc").arg(&probe.path).output().unwrap();
        assert!(
            String::from_utf8_lossy(&holders.stdout).lines().any(|line| line == "csleep"),
            "the actual worker must inherit its lease: {}",
            String::from_utf8_lossy(&holders.stdout)
        );
        let mut pid = None;
        let worker = String::from_utf8_lossy(&holders.stdout)
            .lines()
            .find_map(|line| {
                if let Some(value) = line.strip_prefix('p') {
                    pid = value.parse::<u32>().ok();
                }
                (line == "csleep").then_some(pid).flatten()
            })
            .unwrap();
        // SAFETY: the rendezvous keeps both fixture processes alive here.
        assert_eq!(
            unsafe { libc::getpgid(worker as libc::pid_t) },
            unsafe { libc::getpgid(probe.child.as_ref().unwrap().id() as libc::pid_t) },
            "workers must stay in the runner's timeout process group"
        );
        std::fs::write(rendezvous.path().join("release"), "").unwrap();
        let output = probe.child.take().unwrap().wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if panics {
            assert!(
                !output.status.success() && stderr.contains("deliberate fixture teardown failure"),
                "{scenario}: {stderr}"
            );
        } else {
            assert!(output.status.success(), "{scenario}: {stderr}");
        }
        if std::env::var_os("CONVOY_FIXTURE_TIMINGS").is_some() {
            eprint!("{stderr}");
        }
        assert!(
            probe.observer.try_lock().is_ok(),
            "completed {scenario} workers still retain the inherited lease"
        );
        let state = std::process::Command::new("ps")
            .args(["-p", &unrelated.id().to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout);
        assert!(
            !state.trim().is_empty() && !state.trim().starts_with('Z'),
            "fixture cleanup killed an unrelated process: {state}"
        );
    }
}

#[test]
fn an_unclassified_refusal_waits_for_the_operator_without_pausing_unrelated_work() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| {
        orca["prompt_blocked"] = json!("organization disabled subscription access")
    });
    world.afk(&["nudge", "7", "--instance", "alpha", "--host", "h1", "--now", "1300"]).error();
    tick(&world, 1300);
    tick(&world, 1600);
    assert_eq!(world.claims().len(), 1);
    assert_eq!(world.paused(), json!([]));
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1700"]).json;
    assert_eq!(
        doctor["host"]["host_kit"]["worker_health"]["workers"][0]["action"],
        "waiting for operator"
    );
    assert!(
        !doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["what"].as_str().unwrap().contains("harness is down"))
    );
}

#[test]
fn a_verified_account_fault_holds_new_work_when_no_successor_is_available() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state":"idle","error":{"code":"organization_disabled"}});
        }
    });
    tick(&world, 1300);
    world.issue(9, &["ready-for-agent"]);
    let plan = world.afk(&["plan", "--now", "1400"]).ok();
    assert!(plan["frontier"].as_array().unwrap().is_empty());
    assert_eq!(plan["host_kit"]["worker_health"]["faults"][0]["scope"], "account");
    assert_eq!(world.claims().len(), 1);
    assert_eq!(world.paused(), json!([]), "health does not rewrite the kit pause list");
}

#[test]
fn waiting_for_the_operator_is_named_in_doctor_and_escalates_after_an_hour_on_unchanged_evidence() {
    let world = fleet(1000);
    world.hold_every_harness();
    report(&world, "done", 1000, 1000);
    show(&world, &["Ran the tests.", "Shall I open the PR? [y/n]"]);

    for now in [1300, 1600, 1900, 2200, 2500] {
        tick(&world, now);
    }
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "2501"]).json;
    let worker = &doctor["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "waiting for operator");
    assert_eq!(worker["window_started"], 1300);
    assert_eq!(worker["next_check"], 2800);
    assert_eq!(worker["diagnosis"], Value::Null);
    assert_eq!(worker["path"], json!(worktree(&world)));
    assert_eq!(world.claims().len(), 1);
    assert!(!world.asked_a_harness_about("Shall I open the PR?"), "no harness is asked");
    let named = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["what"].as_str().unwrap().contains("worker health is waiting"))
        .count();
    assert_eq!(named, 1);

    tick(&world, 4900);

    assert_eq!(world.claims(), []);
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    assert_eq!(escalations(&world), 1);
    let comment = escalation_comment(&world);
    assert!(comment.contains("one hour on unchanged evidence"), "{comment}");
    assert!(comment.contains("no Jev key command is set in the host settings"), "{comment}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "4901"]).json;
    assert_eq!(doctor["waiting_on_human"][0]["issue"], 7);
}

/// Every line of the tick logs of `world`.
fn logs(world: &World) -> String {
    std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `worker_health` lines of `logs`, each as its command's fields.
fn health_lines(logs: &str) -> Vec<Value> {
    logs.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|line| line["command"].clone())
        .filter(|command| command["command"] == "worker_health")
        .collect()
}

/// Whether a file under `dir`, but `allowed`, holds `marker`.
fn leaked(dir: &std::path::Path, marker: &[u8], allowed: &std::path::Path) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            leaked(&path, marker, allowed)
        } else {
            path != allowed
                && std::fs::read(&path)
                    .is_ok_and(|bytes| bytes.windows(marker.len()).any(|window| window == marker))
        }
    })
}

#[test]
fn one_jev_request_classes_a_real_finished_screen_and_the_key_goes_nowhere_but_typesafe() {
    let world = fleet(1000);
    world.jev_key_command("printf 'jev-secret-%s' 'marker'");
    let (screen, response) = world.jev_recorded("finished screen");
    report(&world, "done", 1000, 1000);
    show(&world, &screen.iter().map(String::as_str).collect::<Vec<_>>());

    let ticked = [tick(&world, 1300), tick(&world, 1600)];

    let plan = world.afk(&["plan", "--now", "1601"]).ok();
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1601"]).json;
    for printed in ticked.iter().chain([&plan, &doctor]) {
        assert!(!printed.to_string().contains("jev-secret-marker"), "the key is in no output");
    }
    let worker = &plan["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["source"], "jev");
    assert_eq!(worker["action"], "complete");
    assert_eq!(worker["diagnosis"]["class"], "complete");
    assert_eq!(worker["diagnosis"]["scope"], "worker");
    assert_eq!(worker["diagnosis"]["action"], "complete");
    let evidence = &response["answers"]["evidence"]["choice"];
    assert_eq!(worker["diagnosis"]["evidence"], json!([evidence]));
    assert_eq!(worker["judgment"]["response"], response);
    assert!(!world.asked_a_harness_about(evidence.as_str().unwrap()), "no harness is asked");

    let payload = world.typesafe_payload();
    assert_eq!(payload["model"], "jev-latest");
    assert_eq!(payload["state"], screen.join("\n"));
    assert_eq!(payload["questions"].as_object().unwrap().len(), 8);
    assert!(payload["questions"]["evidence"]["criteria"].get(evidence.as_str().unwrap()).is_some());
    assert_eq!(worker["judgment"]["request"], payload, "the request is kept as it was sent");
    let (arguments, config) = world.typesafe_sent();
    assert!(config.contains("url = \"https://api.typesafe.ai/v1/systemone\""), "{config}");
    assert!(config.contains("Authorization: Bearer jev-secret-marker"), "{config}");
    assert!(!arguments.contains("jev-secret-marker"), "the key is in no argument: {arguments}");

    let logs = logs(&world);
    let logged = health_lines(&logs)
        .into_iter()
        .filter_map(|line| serde_json::from_str::<Value>(line["outcome"].as_str()?).ok())
        .find(|outcome| outcome["source"] == "jev")
        .expect("the tick log holds the judgment");
    assert_eq!(logged["judgment"]["request"], payload);
    assert_eq!(logged["judgment"]["response"], response);
    assert!(!leaked(&world.root, b"jev-secret-marker", &world.root.join("typesafe-request")));
    assert!(!logs.contains("jev-secret-marker"));
}

#[test]
fn a_failed_or_silent_key_command_is_an_error_and_the_worker_waits_for_the_operator() {
    for (command, error) in [
        ("exit 3", "the Jev key command failed"),
        ("printf ''", "the Jev key command printed no key"),
    ] {
        let world = fleet(1000);
        world.jev_key_command(command);
        report(&world, "done", 1000, 1000);
        show(&world, &["Ran the tests.", "Shall I open the PR? [y/n]"]);
        tick(&world, 1300);
        tick(&world, 1600);

        assert!(!world.root.join("typesafe-request").exists(), "{command}: nothing is sent");
        assert!(!world.asked_a_harness_about("Shall I open the PR?"), "{command}");
        let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1601"]).json;
        let worker = &doctor["host"]["host_kit"]["worker_health"]["workers"][0];
        assert_eq!(worker["action"], "waiting for operator", "{command}");
        assert!(worker["error"].as_str().unwrap().contains(error), "{command}: {worker}");
        let finding = doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["what"].as_str().unwrap().contains("worker health is waiting"))
            .unwrap_or_else(|| panic!("{command}: the operator is told"));
        assert!(finding["evidence"].as_str().unwrap().contains(error), "{command}: {finding}");
        assert!(finding["fix"].as_str().unwrap().starts_with("orca terminal read"), "{finding}");
        let failed = health_lines(&logs(&world)).iter().any(|line| {
            line["failed"] == true && line["outcome"].as_str().unwrap().contains(error)
        });
        assert!(failed, "{command}: the tick log holds the error");
    }
}

#[test]
fn an_unsettled_judgment_waits_for_the_operator_with_the_screen_line_and_the_reason() {
    let world = fleet(1000);
    world.jev_key_command("printf key");
    let (screen, mut response) = world.jev_recorded("working screen");
    response["answers"]["usage_limit"]["noul"] = json!(0.5);
    world.typesafe_answers(&response);
    report(&world, "done", 1000, 1000);
    show(&world, &screen.iter().map(String::as_str).collect::<Vec<_>>());
    tick(&world, 1300);
    tick(&world, 1600);

    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1601"]).json;
    let worker = &doctor["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "waiting for operator");
    assert_eq!(worker["error"], Value::Null, "an unsettled answer is no error");
    assert_eq!(world.claims().len(), 1);
    let finding = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["what"].as_str().unwrap().contains("worker health is waiting"))
        .expect("the operator is told");
    let evidence = finding["evidence"].as_str().unwrap();
    assert!(evidence.contains("`usage_limit` unsettled, at 0.500"), "{evidence}");
    let last = screen.iter().rev().find(|line| !line.trim().is_empty()).unwrap();
    assert!(evidence.contains(last.as_str()), "{evidence}");
}

#[test]
fn run_sets_the_jev_key_command_from_a_flag_or_the_environment_and_default_removes_it() {
    let world = World::new();
    let command = "printf 'jev-secret-%s' 'marker'";
    let ran = world.afk(&["run", "--jev-key-command", command]).ok();
    assert_eq!(ran["settings"]["jev_key_command"]["to"], command);
    world.afk(&["stop", "--all"]).ok();
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["settings"]["said"]["jev_key_command"], command);
    assert!(!doctor.to_string().contains("jev-secret-marker"));

    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("AFKCONVOY_JEV_KEY_COMMAND", command)
        .args(["run"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    world.afk(&["stop", "--all"]).ok();
    assert_eq!(world.checkout_list()["settings"]["jev_key_command"], command);

    let removed = world.afk(&["run", "--jev-key-command", "default"]).ok();
    assert_eq!(removed["settings"]["jev_key_command"]["to"], Value::Null);
    world.afk(&["stop", "--all"]).ok();
    assert_eq!(world.checkout_list()["settings"]["jev_key_command"], Value::Null);
}

fn tell(world: &World, host: &str, text: &str) -> common::Answer {
    world.afk(&["tell", "7", "--text", text, "--host", host, "--now", "1100"])
}

/// The comments that hold an operator's note.
fn notes(world: &World) -> Vec<String> {
    let comments = world.comments(7).into_iter();
    comments.filter(|body| body.starts_with("<!--convoy:tell-->")).collect()
}

fn told_record(world: &World) -> PathBuf {
    let dir = worktree(world);
    PathBuf::from(common::git(&dir, &["rev-parse", "--absolute-git-dir"])).join("convoy-told")
}

#[test]
fn a_note_is_told_to_a_busy_worker_only_once_its_harness_gave_it_to_the_model() {
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    world.with_orca(|orca| orca["notes"] = json!("queued"));
    let error = tell(&world, "h1", "the body changed:\nuse the v2 endpoint").error();
    let [note] = notes(&world).try_into().unwrap();
    assert!(note.contains("the body changed:\nuse the v2 endpoint"), "{note}");
    let url = world.comment_url(7, &note);
    assert!(error.contains("not delivered") && error.contains(&url), "{error}");
    assert!(error.contains("no second line"), "{error}");
    assert_eq!(typed(&world).len(), 2, "the brief, then the note once");
    let line = &typed(&world)[1];
    assert!(line.contains(&url) && line.contains("the body changed: use the v2 endpoint"));
    assert!(!told_record(&world).exists(), "a queued note is not told");

    // Rerun once the harness takes what it is typed: the same comment.
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let told = tell(&world, "h1", "the body changed:\nuse the v2 endpoint").ok();
    assert_eq!(told["action"], "told");
    assert_eq!(told["comment"], url);
    assert!(told["terminal"].is_string(), "{told}");
    assert_eq!(notes(&world).len(), 1, "no second comment for the same note");
    let record = std::fs::read_to_string(told_record(&world)).unwrap();
    assert_eq!(record, format!("1100 {url}\n"));

    // Told already: the session holds it, and nothing more is typed.
    let typed_before = typed(&world).len();
    let again = tell(&world, "h1", "the body changed:\nuse the v2 endpoint").ok();
    assert_eq!(again["typed"], false, "{again}");
    assert_eq!(typed(&world).len(), typed_before);
    assert_eq!(notes(&world).len(), 1);
}

/// What was sent at the worker of issue 7, each `{text, enter}`.
fn sends(world: &World) -> Vec<Value> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let worker =
        terminals.iter().find(|terminal| terminal["command"] == common::trusted("agent")).unwrap();
    worker["sent"].as_array().unwrap().clone()
}

/// The chat file Grok writes once it gave a typed line to its model.
fn grok_chat(world: &World) -> Option<String> {
    let cwd = worktree(world);
    let files =
        afkconvoy_core::harness::Harness::Grok.note_files(&cwd.to_string_lossy(), "x", true, 0);
    let path = world.home().join(".grok").join(&files[0].path);
    std::fs::read_to_string(path).ok()
}

/// The worker's terminal is Grok, busy, showing `screen`.
fn grok_busy(world: &World, screen: &[&str]) {
    report(world, "working", 1000, 1080);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["command"] == common::trusted("agent") {
                terminal["agent"]["agentType"] = json!("grok");
                terminal["screen"] = json!(screen);
            }
        }
    });
}

#[test]
fn a_busy_grok_is_told_by_send_now_once_the_line_is_queued() {
    let world = fleet(1000);
    world.with_orca(|orca| orca["notes"] = json!("send-now"));
    // As a busy Grok showed it on 2026-11-01: the typed line sits under
    // Queued, and the hint names Enter.
    grok_busy(
        &world,
        &["reading the repo", "▾ Queued 1", "#1 Operator's note …", "Enter:send now"],
    );
    let told = tell(&world, "h1", "the body changed:\nuse the v2 endpoint").ok();
    assert_eq!(told["action"], "told", "{told}");
    assert!(told["seconds"].is_u64(), "{told}");
    assert!(told["typed"].is_null(), "{told}");
    let url = told["comment"].as_str().unwrap();
    let sent = sends(&world);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent[1]["text"].as_str().unwrap().contains(url), "{sent:?}");
    assert!(sent[1]["text"].as_str().unwrap().contains("use the v2 endpoint"), "{sent:?}");
    assert_eq!(sent[1]["enter"], true);
    assert_eq!(sent[2]["text"], "");
    assert_eq!(sent[2]["enter"], true, "send now is Enter alone");
    let chat = grok_chat(&world).expect("the session holds the user message");
    assert!(chat.contains(url) && chat.contains("\"role\":\"user\""), "{chat}");
    assert_eq!(notes(&world).len(), 1);

    let again = tell(&world, "h1", "the body changed:\nuse the v2 endpoint").ok();
    assert_eq!(again["typed"], false, "{again}");
    assert_eq!(again["action"], "told");
    assert_eq!(sends(&world).len(), 3, "a rerun types nothing");
    assert_eq!(notes(&world).len(), 1);
}

#[test]
fn a_busy_grok_whose_line_is_not_queued_is_not_sent_now() {
    let world = fleet(1000);
    world.with_orca(|orca| orca["notes"] = json!("queued"));
    grok_busy(&world, &["still reading the issue"]);
    let error = tell(&world, "h1", "use the v2 endpoint").error();
    assert!(error.contains("not delivered") && error.contains("no second line"), "{error}");
    let sent = sends(&world);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent.iter().all(|row| !row["text"].as_str().unwrap().is_empty()), "{sent:?}");
    assert!(grok_chat(&world).is_none(), "a queued line is not a user message");
}

#[test]
fn a_note_is_told_to_a_stopped_worker_the_same_way() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let told = tell(&world, "h1", "land after #9").ok();
    assert_eq!(told["action"], "told", "{told}");
    assert_eq!(typed(&world).len(), 2);
    assert!(!nudge_record(&world).exists(), "a note is no nudge");
}

#[test]
fn with_no_worker_of_this_host_at_work_a_note_is_only_commented_and_says_why() {
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let elsewhere = tell(&world, "h2", "use the v2 endpoint").ok();
    assert_eq!(elsewhere["action"], "commented");
    assert_eq!(elsewhere["host"], "h1");
    assert!(elsewhere["why"].as_str().unwrap().contains("alpha"), "{elsewhere}");
    assert_eq!(elsewhere["comment"], world.comment_url(7, &notes(&world)[0]));
    assert_eq!(typed(&world).len(), 1, "nothing typed");

    world.issue(8, &["bug"]);
    let unclaimed = world.afk(&["tell", "8", "--text", "hello", "--host", "h1"]).ok();
    assert_eq!(unclaimed["action"], "commented");
    assert!(unclaimed["why"].as_str().unwrap().contains("not claimed"), "{unclaimed}");
    assert!(unclaimed["host"].is_null());
    assert_eq!(typed(&world).len(), 1);
}

fn later_comment(world: &World, text: &str, at: u64) -> String {
    let url = world.comment(7, text);
    world.with_gh(|gh| {
        gh["comments"]["7"].as_array_mut().unwrap().last_mut().unwrap()["at"] = json!(at);
    });
    url
}

fn user_notes(world: &World) -> String {
    use afkconvoy_core::harness::{Harness, SessionLayout};
    let cwd = worktree(world);
    let place = Harness::ClaudeCode.note_place(&cwd.to_string_lossy());
    let SessionLayout::Files { dir, .. } = place.layout else { panic!("Claude message layout") };
    std::fs::read_to_string(
        world.home().join(place.under_home).join(dir).join("afkconvoy-note.jsonl"),
    )
    .unwrap_or_default()
}

#[test]
fn later_issue_comments_are_told_once_with_user_session_proof() {
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let url = later_comment(&world, "Use the v2 endpoint.\nKeep the retries.", 1080);
    let told = tick(&world, 1090);
    assert!(told["errors"].as_array().unwrap().is_empty(), "{told}");
    assert!(
        told["did"].as_array().unwrap().iter().any(|step| {
            step["do"] == "tell" && step["comment"] == url && step["outcome"] == "done"
        }),
        "{told}"
    );
    let session = user_notes(&world);
    assert!(afkconvoy_core::harness::Harness::ClaudeCode.holds_note(&session, &url), "{session}");
    assert!(typed(&world)[1].contains("Use the v2 endpoint. Keep the retries."));
    assert!(typed(&world)[1].contains(&url));
    tick(&world, 1180);
    assert_eq!(typed(&world).len(), 2, "the brief and one update, across tick processes");
}

#[test]
fn later_issue_body_edits_are_told_with_the_whole_new_body() {
    let world = fleet(1000);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let new_body = "# Corrected requirement\nUse v2 and preserve 日本語.";
    world.with_gh(|gh| gh["issues"][0]["body"] = json!(new_body));
    let told = tick(&world, 1090);
    assert!(told["errors"].as_array().unwrap().is_empty(), "{told}");
    let line = &typed(&world)[1];
    assert!(
        line.contains("Whole new body: # Corrected requirement Use v2 and preserve 日本語."),
        "{line}"
    );
    assert!(line.contains("https://gh/i/7"));
    assert!(
        afkconvoy_core::harness::Harness::ClaudeCode
            .holds_note(&user_notes(&world), "#convoy-body-1000-1")
    );
    tick(&world, 1180);
    assert_eq!(typed(&world).len(), 2);
}

#[test]
fn a_stopped_worker_is_told_a_later_comment_before_the_tick_judges_it() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let url = later_comment(&world, "Keep going with v2.", 1080);
    let told = tick(&world, 1090);
    assert!(told["errors"].as_array().unwrap().is_empty(), "{told}");
    assert!(afkconvoy_core::harness::Harness::ClaudeCode.holds_note(&user_notes(&world), &url));
    assert_eq!(typed(&world).len(), 2);
    assert!(!nudge_record(&world).exists(), "a comment is no nudge");
}

#[test]
fn a_body_edit_shows_changed_lines_against_the_workers_starting_body() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.with_gh(|gh| gh["issues"][0]["body"] = json!("# Requirement\nUse v1.\nKeep retries."));
    tick(&world, 1000);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    world.with_gh(|gh| gh["issues"][0]["body"] = json!("# Requirement\nUse v2.\nKeep retries."));
    let told = tick(&world, 1090);
    assert!(told["errors"].as_array().unwrap().is_empty(), "{told}");
    assert_eq!(typed(&world).len(), 2);
    let line = &typed(&world)[1];
    assert!(line.contains("Changed lines") && line.contains("- Use v1. + Use v2."), "{line}");
    assert!(!line.contains("Keep retries"), "unchanged context is omitted: {line}");
}

#[test]
fn a_claim_with_no_worker_types_no_comment_and_dispatches_normally() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.afk(&["claim", "7", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
    let url = later_comment(&world, "Read this at your start.", 1080);
    let started = tick(&world, 1090);
    assert!(started["errors"].as_array().unwrap().is_empty(), "{started}");
    assert!(!started["did"].as_array().unwrap().iter().any(|step| step["do"] == "tell"));
    assert_eq!(typed(&world).len(), 1, "only its brief is typed");
    assert!(!typed(&world)[0].contains(&url));
    assert!(user_notes(&world).is_empty());
}

#[test]
fn a_new_body_edit_keeps_the_revision_still_queued_for_the_worker() {
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    world.with_orca(|orca| orca["notes"] = json!("queued"));
    world.with_gh(|gh| gh["issues"][0]["body"] = json!("First correction."));
    let first = tick(&world, 1090);
    assert_eq!(first["errors"][0]["do"], "tell");
    assert_eq!(typed(&world).len(), 2);
    let earlier = typed(&world)[1].clone();
    world.with_gh(|gh| gh["issues"][0]["body"] = json!("Second correction."));
    afkconvoy::session::record_note(&world.home(), &worktree(&world), &earlier, true, 1100)
        .unwrap();
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let next = tick(&world, 1180);
    assert!(next["errors"].as_array().unwrap().is_empty(), "{next}");
    let told: Vec<&Value> =
        next["did"].as_array().unwrap().iter().filter(|step| step["do"] == "tell").collect();
    assert_eq!(told.len(), 2, "{next}");
    assert_eq!(told[0]["typed"], false, "the first revision was already taken");
    assert_eq!(typed(&world).len(), 3, "each body revision is typed once");
    assert!(typed(&world)[1].contains("First correction."));
    assert!(typed(&world)[2].contains("Second correction."));
}

#[test]
fn a_worker_started_before_upgrade_is_told_a_body_edited_since_its_start() {
    let world = fleet(1000);
    let notes = PathBuf::from(git(&worktree(&world), &["rev-parse", "--absolute-git-dir"]));
    std::fs::remove_file(notes.join("convoy-issue-updates.json")).unwrap();
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    world.with_gh(|gh| {
        gh["issues"][0]["body"] = json!("An edit since the old worker started.");
        gh["issues"][0]["edited"] = json!(1080);
    });
    let told = tick(&world, 1090);
    assert!(told["errors"].as_array().unwrap().is_empty(), "{told}");
    assert_eq!(typed(&world).len(), 2);
    assert!(typed(&world)[1].contains("An edit since the old worker started."));
    tick(&world, 1180);
    assert_eq!(typed(&world).len(), 2);
}

#[test]
fn later_fleet_comments_and_earlier_comments_are_not_told() {
    let world = fleet(1000);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    later_comment(&world, "already read", 999);
    later_comment(&world, "at start", 1000);
    later_comment(&world, "prefix\n<!--convoy:tell-->\noperator note", 1080);
    tick(&world, 1090);
    assert_eq!(typed(&world).len(), 1);
}

#[test]
fn a_comment_written_by_this_worker_is_not_told_even_with_the_same_login() {
    use afkconvoy_core::harness::{Harness, SessionLayout};
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let own = later_comment(&world, "I opened the follow-up issue.", 1080);
    let place = Harness::ClaudeCode.note_place(&worktree(&world).to_string_lossy());
    let SessionLayout::Files { dir, .. } = place.layout else { unreachable!() };
    let session = world.home().join(place.under_home).join(dir).join("worker-writes.jsonl");
    let call = json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "id": "write-1", "name": "Bash", "input": {
            "command": "gh issue comment 7 --body-file reply.md"}}]}});
    let result = json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "write-1", "content": own}]}});
    std::fs::write(&session, format!("{call}\n{result}\n")).unwrap();
    tick(&world, 1090);
    assert_eq!(typed(&world).len(), 1, "a write receipt, not the shared login, identifies it");
    // Session compaction must not turn an already identified write into a
    // person's comment at the next tick.
    std::fs::remove_file(&session).unwrap();
    let human = later_comment(&world, "Also cover timeouts.", 1100);
    tick(&world, 1180);
    assert_eq!(typed(&world).len(), 2);
    assert!(typed(&world)[1].contains(&human));
}

#[test]
fn later_comments_wait_for_a_worker_of_the_claim_s_host() {
    let world = fleet(1000);
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    later_comment(&world, "Use v2", 1080);
    let elsewhere =
        world.afk(&["tick", "--instance", "beta", "--host", "h2", "--now", "1090"]).ok();
    assert!(elsewhere["errors"].as_array().unwrap().is_empty(), "{elsewhere}");
    assert_eq!(typed(&world).len(), 1, "another host types nothing");
}

#[test]
fn a_queued_issue_update_is_retried_without_retyping_and_doctor_names_its_comment() {
    let world = fleet(1000);
    report(&world, "working", 1000, 1080);
    world.with_orca(|orca| orca["notes"] = json!("queued"));
    let url = later_comment(&world, "Use v2", 1080);
    world.afk_lines(&[
        "run",
        "--foreground",
        "--host",
        "h1",
        "--instance",
        "alpha",
        "--second-ms",
        "0",
        "--ticks",
        "3",
        "--now",
        "1090",
    ]);
    let recorded: Vec<Value> = logs(&world)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| {
            entry["tick"]["answer"].as_object().map(|_| entry["tick"]["answer"].clone())
        })
        .collect();
    assert_eq!(recorded.len(), 3);
    for failed in recorded {
        assert!(
            failed["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| { step["do"] == "tell" && step["comment"] == url }),
            "{failed}"
        );
        assert!(
            failed["did"].as_array().unwrap().iter().any(|step| step["do"] == "heartbeat"),
            "{failed}"
        );
    }
    assert_eq!(typed(&world).len(), 2, "the input queue receives one line across retries");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1271"]).json;
    let finding = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| {
            finding["what"].as_str().unwrap_or("").contains("last 3 ticks")
                && finding["evidence"].as_str().unwrap_or("").contains(&url)
        })
        .unwrap_or_else(|| panic!("{doctor}"));
    assert!(finding["what"].as_str().unwrap().contains("#7"));

    // The queued input eventually becomes the model's user message.
    let line = typed(&world)[1].clone();
    afkconvoy::session::record_note(&world.home(), &worktree(&world), &line, true, 1280).unwrap();
    world.afk_lines(&[
        "run",
        "--foreground",
        "--host",
        "h1",
        "--instance",
        "alpha",
        "--second-ms",
        "0",
        "--ticks",
        "1",
        "--now",
        "1360",
    ]);
    assert_eq!(typed(&world).len(), 2);
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1361"]).json;
    assert!(
        !doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| { finding["evidence"].as_str().unwrap_or("").contains(&url) }),
        "{doctor}"
    );
}

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
    // Every hand-off to a human, as before the stock consult: what a worker
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

fn act(world: &World, command: &str, instance: &str, more: &[&str]) -> common::Answer {
    let mut args = vec![command, "--issue", "7", "--instance", instance, "--now", "1100"];
    args.extend(more);
    world.afk(&args)
}

/// The worker's terminal reports `state` since `since`, its last output at `output` (epoch seconds).
fn report(world: &World, state: &str, since: u64, output: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["command"].as_str().is_some_and(|command| command.starts_with("agent")) {
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

    // `working`, but no output within the grace period: a stop report that was lost
    report(&world, "working", 1000, 1080);
    assert_eq!(did(&tick(&world, 1080 + 300))[0], "judge 7 done");
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
    assert_eq!(world.comments(7).len(), 3, "…and nothing is commented");
}

#[test]
fn already_satisfied_with_commits_ahead_and_an_equal_tree_closes_the_issue() {
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
        assert_eq!(world.state(7), "closed");
        assert_eq!(world.claims(), []);
        assert!(!dir.exists());
        assert_eq!(escalations(&world), 0);
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
        let said = declare(&world, "already-satisfied");

        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.state(7), "open");
        assert_eq!(world.claims(), []);
        assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
        assert!(dir.exists(), "the worktree is evidence");
        assert_eq!(world.open_terminals(), Vec::<String>::new(), "its worker is ended");
        // the branch record, the verdict, the escalation
        let comments = world.comments(7);
        assert_eq!(comments.len(), 3);
        assert!(comments[2].starts_with("<!--convoy:escalation claim="), "{}", comments[2]);
        assert!(comments[2].contains("escalated to a human"));
        assert!(comments[2].contains("its branch holds work") && comments[2].contains(&said));
        let file = if dirty { "scratch.txt" } else { "work.txt" };
        assert!(comments[2].contains(file), "{}", comments[2]);
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
        let comments = world.comments(7);
        assert!(comments[2].contains(says) && comments[2].contains(&said), "{}", comments[2]);
        assert!(!dir.exists(), "a worktree whose branch holds no work is removed");
        // off the frontier: nothing dispatches it again
        assert_eq!(did(&tick(&world, 1180)), [] as [String; 0]);
    }
}

/// What was typed at the worker of issue 7.
fn typed(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let worker = terminals
        .iter()
        .find(|terminal| {
            terminal["command"].as_str().is_some_and(|command| command.starts_with("agent"))
        })
        .unwrap();
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
    "⏺ Monitor(gate --train finishing for #7)",
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
    assert!(!world.root.join("diagnosis-asked").exists());

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
    assert!(world.comments(7)[1].contains("the schema needs a decision"));
    let plan = world.afk(&["plan", "--now", "1700"]).ok();
    let waiting = &plan["waiting_on_human"][0];
    assert_eq!(waiting["issue"], 7);
    assert_eq!(waiting["why"], "escalated: the schema needs a decision");
    assert_eq!(waiting["url"], world.comment_url(7, &world.comments(7)[1]));
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
    command: std::process::Child,
    _host_runner: common::Running,
}

impl Drop for Queued {
    fn drop(&mut self) {
        let _ = self.command.kill();
        let _ = self.command.wait();
    }
}

fn queued(world: &World, asked: u64) -> Queued {
    let host_runner = world.host_held();
    let ran = world.root.join("queued-ran");
    let script = format!("touch {}; sleep 600", ran.display());
    let command = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &worktree(world))
        .env("AFKCONVOY_ISSUE", "7")
        .args(["--now", &asked.to_string(), "exclusive", "--", "sh", "-c", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ran.exists() {
        assert!(std::time::Instant::now() < until, "the queued command never ran");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Queued { command, _host_runner: host_runner }
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
    assert_eq!(did(&tick(&world, 1250 + 600))[0], "judge 7 waiting");
    assert_eq!(typed(&world).len(), 1, "no nudge");
    assert_eq!(world.claims().len(), 1);
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
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
fn waiting_for_the_operator_never_waits_on_a_diagnosis_or_escalates_after_an_hour() {
    let world = fleet(1000);
    world.hold_every_harness();
    report(&world, "done", 1000, 1000);

    for now in [1300, 1600, 1900, 2200, 2500, 4900] {
        tick(&world, now);
    }
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "5200"]).json;
    let worker = &doctor["host"]["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["action"], "waiting for operator");
    assert_eq!(worker["window_started"], 1300);
    assert_eq!(worker["next_check"], 5200);
    assert_eq!(worker["attempts"], 0);
    assert_eq!(worker["diagnosis"], Value::Null);
    assert_eq!(worker["path"], json!(worktree(&world)));
    assert_eq!(world.claims().len(), 1);
    assert_eq!(escalations(&world), 0);
    assert!(!world.root.join("diagnosis-asked").exists());
    assert!(doctor["findings"].as_array().unwrap().iter().any(|finding| {
        finding["what"].as_str().unwrap().contains("worker health is waiting for the operator")
    }));
}

#[test]
fn one_jev_request_classes_a_finished_screen_and_the_key_never_leaks() {
    let world = fleet(1000);
    world.jev_key_command("printf 'jev-secret-%s' 'marker'");
    world.jev_answers(json!({
        "usage_limit": {"type": "noul", "noul": 0.02},
        "busy": {"type": "noul", "noul": 0.05},
        "finished": {"type": "noul", "noul": 0.97},
        "waiting_for_person": {"type": "noul", "noul": 0.10},
        "trust_prompt": {"type": "noul", "noul": 0.01},
        "login_refused": {"type": "noul", "noul": 0.01},
        "evidence": {"type": "choice", "choice": "Finished: opened the PR.",
            "confidence": 0.99, "probabilities": {"Finished: opened the PR.": 0.99}}
    }));
    report(&world, "done", 1000, 1000);
    show(&world, &["Implemented and verified the change.", "Finished: opened the PR."]);

    tick(&world, 1300);
    tick(&world, 1600);

    let plan = world.afk(&["plan", "--now", "1601"]).ok();
    let worker = &plan["host_kit"]["worker_health"]["workers"][0];
    assert_eq!(worker["source"], "jev");
    assert_eq!(worker["action"], "complete");
    assert_eq!(worker["diagnosis"]["class"], "complete");
    assert_eq!(worker["diagnosis"]["scope"], "worker");
    assert_eq!(worker["diagnosis"]["action"], "complete");
    assert_eq!(worker["diagnosis"]["evidence"], json!(["Finished: opened the PR."]));
    assert_eq!(worker["judgment"]["answers"]["answers"]["finished"]["noul"], 0.97);
    assert!(!world.root.join("diagnosis-asked").exists());

    let payload = std::fs::read_to_string(world.root.join("typesafe-payload")).unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["state"], "Implemented and verified the change.\nFinished: opened the PR.");
    assert_eq!(payload["questions"].as_object().unwrap().len(), 7);
    assert!(payload["questions"]["evidence"]["criteria"].get("Finished: opened the PR.").is_some());
    let request = std::fs::read_to_string(world.root.join("typesafe-request")).unwrap();
    assert!(request.contains("Authorization: Bearer jev-secret-marker"), "{request}");
    let logs = std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logs.contains("jev-latest") && logs.contains("0.97"), "{logs}");

    fn leaked(dir: &std::path::Path, marker: &[u8], allowed: &std::path::Path) -> bool {
        std::fs::read_dir(dir).unwrap().any(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                leaked(&path, marker, allowed)
            } else {
                path != allowed
                    && std::fs::read(&path).is_ok_and(|bytes| {
                        bytes.windows(marker.len()).any(|window| window == marker)
                    })
            }
        })
    }
    assert!(!leaked(&world.root, b"jev-secret-marker", &world.root.join("typesafe-request")));
    assert!(!logs.contains("jev-secret-marker"));
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

use crate::common;

use std::path::PathBuf;

use common::{World, commit, git};
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args =
        ["tick", "--instance", "alpha", "--host", "h1", "--now", &now, "--worker-command", "agent"];
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
            if terminal["command"] == "agent" {
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
        ["judge 7 done", "judge 9 nudged", "dispatch 10 done", "heartbeat done"],
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
    let worker = terminals.iter().find(|terminal| terminal["command"] == "agent").unwrap();
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
    let args = ["run", "--host", "h1", "--worker-command", "agent", "--second-ms", "0"];
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
    assert_eq!(read_limits(&world), ["12"], "its footer is read, and only that");
    let ticked = tick(&world, 1800);
    assert_eq!(did(&ticked), ["judge 7 waiting", "heartbeat done"]);
    assert_eq!(ticked["did"][0]["waiting_since"], 1300, "since when it was first seen");
    assert_eq!(world.claims().len(), 1);
    // the tick's line says so, and since when
    let lines = run_line(&world, 1820);
    let said = "waiting on its own background work #7 (since 1970-01-01 00:21 UTC, 8m ago)";
    assert!(lines.iter().any(|line| line.contains(said)), "{lines:?}");

    // the work ended: the silence is counted as ever — a nudge, then stalled past it
    show(&world, &ENDED);
    assert_eq!(did(&tick(&world, 1900)), ["judge 7 nudged", "heartbeat done"]);
    assert_eq!(typed(&world).len(), 2);
    assert_eq!(did(&tick(&world, 2199)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 2200)), ["judge 7 done"]);
    let said = world.comments(7).last().unwrap().clone();
    assert!(said.contains("stayed silent after one nudge"), "{said}");
}

#[test]
fn a_worker_waiting_past_the_gate_limit_is_silent_saying_so() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &WAITING);
    assert_eq!(did(&tick(&world, 1300))[0], "judge 7 waiting");
    // the gate limit is 600s here: still waiting just short of it…
    assert_eq!(did(&tick(&world, 1300 + 599))[0], "judge 7 waiting");
    // …and silent at it, never nudged
    assert_eq!(did(&tick(&world, 1300 + 600))[0], "judge 7 done");
    assert_eq!(typed(&world).len(), 1, "no nudge");
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    let said = world.comments(7).last().unwrap().clone();
    assert!(said.contains("waited 600s on its own background work, past the limit"), "{said}");
    assert!(said.contains("1 shell, 1 monitor"), "where it stopped: {said}");
}

fn nudge_record(world: &World) -> PathBuf {
    let dir = worktree(world);
    PathBuf::from(common::git(&dir, &["rev-parse", "--absolute-git-dir"])).join("convoy-nudged")
}

#[test]
fn a_silent_worker_is_nudged_once_then_escalated_with_where_it_stopped() {
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

    // within grace of stopping: left
    assert_eq!(did(&tick(&world, 1090)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(typed(&world).len(), 1, "only its brief");

    // past grace, having declared nothing: told once to carry on
    let ticked = tick(&world, 1300);
    assert_eq!(did(&ticked), ["judge 7 nudged", "heartbeat done"]);
    let lines = typed(&world);
    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with("You stopped without an outcome. Nobody is watching"));
    assert!(lines[1].contains("convoy-worker-prompt.md") && lines[1].contains("convoy:verdict"));
    assert_eq!(std::fs::read_to_string(nudge_record(&world)).unwrap(), "1300");
    assert_eq!(world.claims().len(), 1, "the claim is still held");
    assert_eq!(read_limits(&world), ["12"], "only its footer is read to nudge");

    // the nudge is a sign of life for one grace period
    assert_eq!(did(&tick(&world, 1599)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(typed(&world).len(), 2);
    assert_eq!(screen_reads(&world), [] as [Vec<String>; 0]);

    // still silent: escalated, saying where it stopped
    assert_eq!(did(&tick(&world, 1600)), ["judge 7 done"]);
    assert_eq!(typed(&world).len(), 2, "never a second nudge");
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    assert!(dir.exists(), "the worktree holds its work");
    let comments = world.comments(7);
    let said = comments.last().unwrap();
    assert!(said.contains("stayed silent after one nudge"), "{said}");
    let screen = "```\nRan the tests.\nShall I open the PR? [y/n]\n```";
    // …and ending with the line that answers it, the issue's number filled in
    let answer = "- fix what the comment names, and put the ready label back — `gh issue edit 7 \
                  --add-label ready-for-agent --remove-label ready-for-human`\n- close it: the \
                  fleet leaves a closed issue alone — `gh issue close 7`";
    // …then the work only this host holds, each commit the remote lacks by its subject…
    let only_here = format!(
        "**Work only one host holds**: the worktree `{}` on `h1` is kept — it holds 1 commit \
         the remote does not have.\n- work.txt",
        dir.display()
    );
    assert!(said.contains(&format!("{screen}\n\n{only_here}\n\n**To answer it**")), "{said}");
    assert!(said.trim_end().ends_with(answer), "{said}");
    // An unbound repo's escalated issue is excluded as escalated, and waits for a human.
    let plan = world.afk(&["plan", "--now", "1700"]).ok();
    assert_eq!(plan["excluded"][0]["reason"], "escalated: waits for the ready label", "{plan}");
    let waiting = &plan["waiting_on_human"];
    assert_eq!((&waiting[0]["issue"], &waiting[0]["why"]), (&json!(7), &json!("escalated")));
    assert_eq!(
        waiting[0]["answer"],
        "gh issue edit 7 --add-label ready-for-agent --remove-label ready-for-human"
    );
    let reads = screen_reads(&world);
    assert_eq!(reads.len(), 2, "its footer, then where it stopped: {reads:?}");
    let limit = reads[1].iter().position(|arg| arg == "--limit").unwrap();
    assert_eq!(
        (reads[1][limit + 1].as_str(), reads[1].contains(&"--screen".to_string())),
        ("60", true)
    );
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
    assert_eq!(did(&tick(&world, 1300))[0], "judge 7 nudged");
    declare(&world, "already-satisfied");
    assert_eq!(did(&tick(&world, 1600)), ["judge 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(typed(&world).len(), 2);
}

#[test]
fn a_nudge_the_worker_did_not_take_is_not_recorded() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| orca["send_refused"] = json!(true));
    let ticked = tick(&world, 1300);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert!(!nudge_record(&world).exists());

    // so the next tick nudges, and the silence is not held against the worker
    world.with_orca(|orca| orca["send_refused"] = json!(false));
    assert_eq!(did(&tick(&world, 1390))[0], "judge 7 nudged");
    assert_eq!(std::fs::read_to_string(nudge_record(&world)).unwrap(), "1390");
    // a terminal that cannot be read does not hold the escalation back
    world.with_orca(|orca| orca["read_fails"] = json!(true));
    assert_eq!(did(&tick(&world, 1700)), ["judge 7 done"]);
    assert!(world.comments(7).last().unwrap().contains("after one nudge.\n\n**To answer it**"));
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

/// Puts a command of the worker's in the host's `exclusive` queue, asked at
/// `asked`: held for as long as this test runs.
fn queued(world: &World, asked: u64) {
    let queue = afkconvoy::exclusive::Queue::at(world.cache().join("afkconvoy").join("exclusive"));
    let dir = std::fs::canonicalize(worktree(world)).unwrap();
    let ticket = afkconvoy_core::exclusive::Ticket {
        number: 0,
        pid: std::process::id(),
        command: "cargo test --workspace".into(),
        dir: dir.join("crates").display().to_string(),
        issue: Some(7),
        asked,
        ran: None,
    };
    let (held, _) = queue.join(ticket).unwrap();
    // Held for the rest of the test, as a live `exclusive` process holds its own.
    std::mem::forget(held);
}

#[test]
fn a_worker_waiting_in_the_exclusive_queue_is_busy_until_the_gate_limit() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    show(&world, &["Running the heavy tests alone.", "❯ "]);
    queued(&world, 1250);

    // past grace, its screen showing nothing running: it waits in the queue
    let ticked = tick(&world, 1300);
    assert_eq!(did(&ticked), ["judge 7 waiting", "heartbeat done"]);
    assert_eq!(ticked["did"][0]["waiting_since"], 1250, "since it asked: {ticked}");
    assert_eq!(did(&tick(&world, 1250 + 599))[0], "judge 7 waiting");
    assert_eq!(typed(&world).len(), 1, "never nudged for it");

    // the gate limit is 600s here: a wait that long is silent, as any such wait is
    assert_eq!(did(&tick(&world, 1250 + 600))[0], "judge 7 done");
    assert_eq!(typed(&world).len(), 1, "no nudge");
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    let said = world.comments(7).last().unwrap().clone();
    assert!(said.contains("waited 600s on its own background work, past the limit"), "{said}");
}

#[test]
fn a_refusing_harness_holds_claims_reports_once_and_recovers_without_restart() {
    let world = fleet(1000);
    report(&world, "done", 1000, 1000);
    world.with_orca(|orca| {
        orca["prompt_blocked"] = json!("organization disabled subscription access")
    });
    for now in [1300, 1390, 1480] {
        world.afk(&[
            "tick",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            &now.to_string(),
            "--worker-command",
            "agent",
        ]);
    }
    assert_eq!(world.claims().len(), 1);
    world.orca_calls();
    for now in [1500, 1600, 1700] {
        let ticked = tick(&world, now);
        assert_eq!(ticked["errors"], json!([]));
        assert_eq!(world.claims().len(), 1);
        assert_eq!(escalations(&world), 0);
    }
    let calls = world.orca_calls();
    assert!(!calls.iter().any(|call| common::asked(call) == "terminal send"));
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1700"]).json;
    let found: Vec<_> = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["what"].as_str().unwrap().contains("harness is down"))
        .collect();
    assert_eq!(found.len(), 1, "{doctor}");
    assert!(found[0]["evidence"].as_str().unwrap().contains("organization disabled"));
    assert!(found[0]["fix"].as_str().unwrap().contains("afkconvoy switch"));
    let files = std::fs::read_dir(world.cache().join("afkconvoy/runs")).unwrap();
    let entries = files
        .filter_map(Result::ok)
        .filter(|file| file.path().extension().is_some_and(|ext| ext == "jsonl"))
        .flat_map(|file| {
            std::fs::read_to_string(file.path())
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .map(|line| serde_json::from_str::<Value>(&line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        entries.iter().filter(|entry| entry["command"]["command"] == "harness_down").count(),
        1,
        "{entries:?}"
    );
    world.with_orca(|orca| orca["prompt_blocked"] = Value::Null);
    let recovered = tick(&world, 1780);
    assert!(did(&recovered).contains(&"judge 7 nudged".to_string()), "{recovered}");
    assert_eq!(world.claims().len(), 1);
    assert_eq!(std::fs::read_to_string(nudge_record(&world)).unwrap(), "1780");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1780"]).json;
    assert!(
        !doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["what"].as_str().unwrap().contains("harness is down"))
    );
}

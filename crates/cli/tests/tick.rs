use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::common;

use common::{World, git};
use serde_json::{Value, json};

fn tick(world: &World, now: &str) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    ticked["did"]
        .as_array()
        .unwrap()
        .iter()
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        // a worker asked after, and left or nudged, is not what these tests are about
        .filter(|step| !(step["do"] == "judge" && step["outcome"] != "done"))
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world with `numbers` ready.
fn backlog(numbers: &[u64]) -> World {
    let world = World::new();
    world.list_checkout(&world.clone);
    for &n in numbers {
        world.issue(n, &["ready-for-agent"]);
    }
    world
}

fn alpha(numbers: &[u64]) -> Vec<(u64, String)> {
    numbers.iter().map(|&n| (n, "alpha".to_string())).collect()
}

#[test]
fn a_tick_dispatches_the_whole_frontier_lowest_number_first() {
    let world = backlog(&[9, 3, 5]);
    let ticked = tick(&world, "1000");
    assert_eq!(
        did(&ticked),
        ["dispatch 3 done", "dispatch 5 done", "dispatch 9 done", "heartbeat done"]
    );
    assert_eq!(ticked["held"], 3);
    assert_eq!(ticked["frontier_remaining"], 0);
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(world.claims(), alpha(&[3, 5, 9]));

    // nothing left: the next tick takes nothing
    let again = tick(&world, "1090");
    assert_eq!(did(&again), ["heartbeat done"]);
    assert_eq!(world.claims(), alpha(&[3, 5, 9]));
}

#[test]
fn a_tick_never_waits_for_the_workers_it_started() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert_eq!(
        calls.iter().filter(|call| *call == "terminal wait").count(),
        2,
        "only for readiness at start and immediately before the brief"
    );
}

#[test]
fn a_brief_waits_for_a_started_harness_whose_prompt_settles_late() {
    // Ready once, then redrawing past a one-second check: on a busy host a
    // harness just started is no failed start.
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["settles_after_ms"] = json!(5000));
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 3 done", "heartbeat done"], "{ticked}");
    assert_eq!(world.claims(), alpha(&[3]));
}

#[test]
fn a_brief_whose_idle_check_times_out_follows_in_the_same_terminal_on_a_later_tick() {
    for timeouts in [json!([2]), json!([2, 3])] {
        let world = backlog(&[3]);
        world.with_orca(|orca| orca["timeout_on_waits"] = timeouts.clone());
        let first = tick(&world, "1000");
        assert_eq!(first["errors"], json!([]), "{first}");
        assert_eq!(world.claims(), alpha(&[3]));
        let before = world.orca();
        let worker = before["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|terminal| terminal["open"] == true)
            .unwrap();
        let handle = worker["handle"].clone();
        assert_eq!(worker["sent"], json!([]), "no line before its prompt settles");
        let calls = world.orca_calls();
        assert_eq!(calls.iter().filter(|call| common::asked(call) == "terminal wait").count(), 2);

        for (i, _) in timeouts.as_array().unwrap().iter().enumerate() {
            let again = tick(&world, &(1090 + i * 90).to_string());
            assert_eq!(again["errors"], json!([]), "{again}");
            assert_eq!(world.claims(), alpha(&[3]));
            assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
            assert!(
                !world.orca_calls().iter().any(|call| common::asked(call) == "terminal create")
            );
        }
        let after = world.orca();
        let worker = after["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|terminal| terminal["handle"] == handle)
            .unwrap();
        let sent = worker["sent"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0]["text"].as_str().unwrap().contains("convoy-worker-prompt.md"));
        tick(&world, "1270");
        assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"));
    }
}

#[test]
fn a_ready_terminal_waiting_for_its_brief_is_not_reused_under_a_later_claim() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["timeout_on_waits"] = json!([2]));
    tick(&world, "1000");
    let before = world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["open"] == true)
        .unwrap()["handle"]
        .clone();
    world.afk(&["release", "--issue", "3", "--instance", "alpha"]).ok();
    world.with_orca(|orca| orca["timeout_on_waits"] = json!([]));
    let again = tick(&world, "1090");
    assert_eq!(again["errors"], json!([]), "{again}");
    let after = world.orca();
    let worker = after["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["open"] == true)
        .unwrap();
    assert_ne!(worker["handle"], before);
}

#[test]
fn a_missing_ready_terminal_is_replaced_without_counting_its_pending_brief_as_a_failed_start() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["timeout_on_waits"] = json!([2]));
    tick(&world, "1000");
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
        orca["never_ready"] = json!(true);
    });
    let failed = tick(&world, "1090");
    assert_eq!(failed["errors"][0]["do"], "judge", "{failed}");
    assert_eq!(world.claims(), alpha(&[3]));
    assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
    tick(&world, "1180");
    assert_eq!(world.claims(), [], "two genuine readiness failures still escalate");
    assert!(world.comments(3).iter().any(|body| body.contains("worker start failed again")));
}

#[test]
fn a_readiness_wait_timeout_is_still_a_failed_start() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["timeout_on_waits"] = json!([1]));
    let failed = tick(&world, "1000");
    assert!(failed["errors"][0]["error"].as_str().unwrap().contains("timeout"), "{failed}");
    assert_eq!(world.claims(), alpha(&[3]));
    tick(&world, "1090");
    assert_eq!(world.claims(), []);
    assert!(world.comments(3).iter().any(|body| body.contains("worker start failed again")));
}

/// A start that failed again escalates its claim, then fails its step: the
/// tick's line says both, the escalation before the failure.
#[test]
fn a_step_that_fails_after_it_escalated_still_says_the_escalation() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["timeout_on_waits"] = json!([1]));
    tick(&world, "1000");
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1090"]);
    assert_eq!(world.claims(), []);
    let line = &lines[0];
    let escalated = line.find("escalated #3 (").unwrap_or_else(|| panic!("{lines:?}"));
    let failed = line.find("FAILED ").unwrap_or_else(|| panic!("{lines:?}"));
    assert!(escalated < failed, "{lines:?}");
    assert!(line.contains("the claim was escalated"), "{lines:?}");
}

#[test]
fn a_brief_waits_for_the_harness_to_end_its_quiet_startup_working_episode() {
    let world = backlog(&[3]);
    world.with_kit(|kit| {
        kit["harnesses"] = json!([{"harness": "codex",
            "command": "codex --dangerously-bypass-approvals-and-sandbox --config tui.animations=false"}]);
    });
    // Every tui-idle wait is satisfied immediately. Only fresh runtime
    // observations see Codex's startup working episode end.
    world.with_orca(|orca| orca["startup_working_reports"] = json!(2));
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 3 done", "heartbeat done"], "{ticked}");
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    let orca = world.orca();
    let terminal =
        orca["terminals"].as_array().unwrap().iter().find(|t| t["open"] == true).unwrap();
    assert_eq!(terminal["startup_observations"], json!(["working", "working", "idle"]));
    assert_eq!(terminal["last_prompt_stages"], json!(["input_accepted", "turn_started"]));
    assert_eq!(terminal["sent"].as_array().unwrap().len(), 1);
    let handle = terminal["handle"].clone();
    world.orca_calls();

    let again = tick(&world, "1090");
    assert_eq!(again["errors"], json!([]), "{again}");
    assert_eq!(world.claims(), alpha(&[3]));
    assert!(
        !world.orca_calls().iter().any(|call| matches!(
            common::asked(call).as_str(),
            "terminal create" | "terminal close" | "terminal send"
        )),
        "the worker that took its brief is never closed, replaced or briefed twice"
    );
    let orca = world.orca();
    let open: Vec<_> =
        orca["terminals"].as_array().unwrap().iter().filter(|t| t["open"] == true).collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["handle"], handle);
}

#[test]
fn a_claim_whose_issue_is_closed_is_settled_and_its_slot_refilled() {
    let world = backlog(&[3, 5]);
    tick(&world, "1000");
    world.issue(9, &["ready-for-agent"]);
    let worktree = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
    let tmp = world.tmp(std::path::Path::new(&worktree));
    assert!(tmp.is_dir(), "its worker's TMPDIR");
    world.close_issue(3);
    world.watch_pushes(None);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done", "dispatch 9 done", "heartbeat done"]);
    assert!(!std::path::Path::new(&worktree).exists(), "the worktree is removed");
    assert!(tmp.symlink_metadata().is_err(), "its worker's TMPDIR with it");
    assert_eq!(world.claims(), alpha(&[5, 9]));
    // the worktree went first, the claim last
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(calls.contains(&"worktree rm".to_string()));
    assert_eq!(world.pushes()[0], "refs/convoy/claim/3");
}

#[test]
fn a_settling_that_failed_keeps_the_claim_and_the_pass_goes_on() {
    let world = backlog(&[3, 5]);
    world.afk(&["dispatch", "--issue", "3", "--instance", "alpha", "--now", "1000"]).ok();
    world.close_issue(3);
    world.with_orca(|orca| orca["rm_fails"] = json!(true));

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 failed", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(ticked["errors"][0]["do"], "settle");
    assert_eq!(ticked["errors"][0]["issue"], 3);
    assert!(ticked["errors"][0]["error"].as_str().unwrap().contains("worktree_busy"));
    // No topic explains what orca refused with: the step names none.
    assert_eq!(ticked["errors"][0].get("see"), None);
    assert_eq!(world.claims(), alpha(&[3, 5]), "the claim is held for the next tick");

    world.with_orca(|orca| orca["rm_fails"] = json!(false));
    assert_eq!(did(&tick(&world, "1180"))[0], "settle 3 done");
}

#[test]
fn a_settled_worktree_goes_when_the_gate_left_a_directory_the_owner_cannot_write() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    let sealed = seal(&worktree);
    let held = world.root.join("held-role");
    std::fs::create_dir(&held).unwrap();
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o555)).unwrap();
    world.close_issue(3);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done"], "{ticked}");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    assert!(!worktree.exists(), "the worktree is removed");
    assert!(!sealed.exists());
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(!common::worktree_listed(&listed, &worktree), "{listed}");
    assert_eq!(world.claims(), []);
    assert_no_removal_error(&world);
    assert_eq!(mode(&held), 0o555, "a role copy outside the worktree stays read-only");
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_later_tick_removes_a_worktree_orca_already_gave_up_on() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    seal(&worktree);
    unregister(&world, &worktree);
    record_failed_removal(&world, &worktree, &head);
    world.close_issue(3);
    world.remote(&["update-ref", "-d", "refs/convoy/claim/3"]);

    let ticked = tick(&world, "1090");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    assert!(!worktree.exists(), "the directory a removal left is gone");
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(!common::worktree_listed(&listed, &worktree), "{listed}");
    assert_no_removal_error(&world);
    assert_eq!(world.claims(), []);
}

#[test]
fn a_worktree_a_process_is_in_waits_until_that_process_is_gone() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    world.close_issue(3);
    let _held = Hold(Command::new("sleep").arg("120").current_dir(&worktree).spawn().unwrap());

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 waiting", "heartbeat done"], "{ticked}");
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(worktree.is_dir(), "a worktree a process is in stays");
    assert_eq!(world.claims(), alpha(&[3]));

    drop(_held);
    let again = tick(&world, "1180");
    assert_eq!(did(&again)[0], "settle 3 done", "{again}");
    assert!(!worktree.exists(), "the next tick removes it once the process is gone");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_settled_worktree_whose_directory_is_already_gone_has_its_registration_removed() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    std::fs::remove_dir_all(&worktree).unwrap();
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(common::worktree_listed(&listed, &worktree), "git still lists it: {listed}");
    world.close_issue(3);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done"], "{ticked}");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(!common::worktree_listed(&listed, &worktree), "{listed}");
    assert!(
        world.orca()["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["path"] != worktree.to_str().unwrap())
    );
    assert_eq!(world.claims(), []);
}

#[test]
fn a_settled_worktree_whose_directory_is_gone_with_a_head_not_on_the_remote_is_kept() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    common::commit(&worktree, "three.txt", "3\n");
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    // orca lists the head it read while the directory was there
    world.with_orca(|orca| {
        for row in orca["worktrees"].as_array_mut().unwrap() {
            if row["path"] == worktree.to_str().unwrap() {
                row["head"] = json!(head);
            }
        }
    });
    std::fs::remove_dir_all(&worktree).unwrap();
    world.close_issue(3);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done"], "{ticked}");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    assert!(
        world.orca()["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["path"] == worktree.to_str().unwrap()),
        "a head the remote does not have keeps its registration"
    );
    assert_eq!(world.claims(), []);
}

/// A gate a scheduler paused, with what it started, both stopped, and the
/// scheduler itself killed: nothing is left to resume them. Dropped, it
/// resumes and kills both, so a failing test leaves neither stopped.
struct Paused {
    gate: u32,
    child: u32,
}

impl Paused {
    fn start(worktree: &Path, marks: &Path) -> Self {
        let script = r#"cd "$1"
sh -c 'sleep 120 & echo $! > "$0/child"; wait' "$2" &
echo $! > "$2/gate"
while ! test -s "$2/child"; do sleep 0.01; done
kill -STOP "$(cat "$2/child")" "$(cat "$2/gate")"
touch "$2/paused"
exec sleep 120"#;
        let mut scheduler =
            Command::new("sh").args(["-c", script, "sh"]).arg(worktree).arg(marks).spawn().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !marks.join("paused").exists() {
            assert!(std::time::Instant::now() < until, "the scheduler never paused its gate");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let pid = |name: &str| -> u32 {
            std::fs::read_to_string(marks.join(name)).unwrap().trim().parse().unwrap()
        };
        let paused = Self { gate: pid("gate"), child: pid("child") };
        // The scheduler goes without a chance to resume what it paused.
        scheduler.kill().unwrap();
        scheduler.wait().unwrap();
        assert_eq!(state(paused.gate).as_deref(), Some("T"), "the gate stays stopped");
        assert_eq!(state(paused.child).as_deref(), Some("T"), "what it started stays stopped");
        paused
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        for pid in [self.child, self.gate] {
            if state(pid).is_some() {
                // SAFETY: these pids are this test's own paused fixture.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGCONT);
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }
}

/// The first letter of `pid`'s state, `None` once it is gone or a zombie.
fn state(pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "stat="]).output().unwrap();
    let state = String::from_utf8_lossy(&out.stdout).trim().chars().next()?;
    (state != 'Z').then(|| state.to_string())
}

#[test]
fn settling_ends_a_gate_its_killed_scheduler_left_stopped_and_removes_the_worktree() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    world.close_issue(3);
    let paused = Paused::start(&worktree, &world.root);

    let began = std::time::Instant::now();
    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked)[0], "settle 3 done", "{ticked}");
    assert!(began.elapsed() < std::time::Duration::from_secs(30), "ended within a bound");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state(paused.gate).is_some() || state(paused.child).is_some() {
        assert!(std::time::Instant::now() < until, "the paused gate's processes still exist");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!worktree.exists(), "the worktree is removed");
    assert_eq!(world.claims(), []);
    let logged: String = std::fs::read_dir(world.cache().join("afkconvoy/runs"))
        .unwrap()
        .flatten()
        .map(|file| std::fs::read_to_string(file.path()).unwrap_or_default())
        .collect();
    for pid in [paused.gate, paused.child] {
        assert!(
            logged.contains(&format!("ended stopped orphan pid {pid},")),
            "what settling ended is recorded: {logged}"
        );
    }
}

#[test]
fn a_landed_worker_idle_at_its_prompt_is_ended_and_its_worktree_removed() {
    let world = backlog(&[3]);
    world.set_gate("true");
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    common::commit(&worktree, "three.txt", "3\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);
    world.pr(30, 3, &git(&worktree, &["symbolic-ref", "--short", "HEAD"]));
    let landed = world.afk_in(&worktree, &["land", "--issue", "3", "--now", "1050"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let session = Session::start(&world, &worktree);
    report(&world, "idle", 1060);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done"], "{ticked}");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    assert!(session.ended(), "the worker's own session is ended");
    assert!(open_terminals(&world, &worktree).is_empty(), "its terminal is closed");
    assert!(!worktree.exists(), "the worktree is removed");
    assert_eq!(world.claims(), []);
}

#[test]
fn settling_waits_for_an_old_worker_still_ending_instead_of_failing_the_tick() {
    let world = backlog(&[3]);
    world.set_gate("true");
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    common::commit(&worktree, "three.txt", "3\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);
    world.pr(30, 3, &git(&worktree, &["symbolic-ref", "--short", "HEAD"]));
    let landed = world.afk_in(&worktree, &["land", "--issue", "3", "--now", "1050"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let _session = Session::start(&world, &worktree);
    report(&world, "idle", 1060);
    world.control_termination_time();
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked)[0], "settle 3 waiting", "{ticked}");
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(worktree.is_dir(), "a worktree whose worker is still ending stays");
    let all =
        world.afk(&["doctor", "--all", "--host", "h1", "--since", "1000", "--now", "1090"]).json;
    let findings = all["checkouts"][0]["doctor"]["findings"].as_array().unwrap();
    assert!(!findings.iter().any(|finding| finding["kind"] == "tick_failed"), "{findings:#?}");
    assert!(!findings.iter().any(|finding| finding["kind"] == "settle_waiting"), "{findings:#?}");

    let late = tick(&world, "1400");
    assert_eq!(late["errors"], json!([]), "{late}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1400"]).json;
    let waited = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["kind"] == "settle_waiting")
        .count();
    assert_eq!(waited, 1, "past the grace, still ending is a finding: {doctor:#}");

    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(false));
    let again = tick(&world, "1490");
    assert_eq!(did(&again)[0], "settle 3 done", "{again}");
    assert!(!worktree.exists(), "once its end is confirmed, a later tick removes it");
    assert_eq!(world.claims(), []);
}

#[test]
fn settling_waits_for_a_finishing_worker_then_leaves_without_a_failed_tick() {
    let world = backlog(&[391]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    let session = Session::start(&world, &worktree);
    report(&world, "working", 1000);
    world.close_issue(391);

    let (lines, _) = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        "1090",
        "--ticks",
        "1",
        "--second-ms",
        "0",
    ]);
    assert!(
        lines.iter().any(|line| line.contains("settling #391 once its worker leaves")),
        "{lines:?}"
    );
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1090"]).json;
    assert!(
        doctor["fleet"]["last_tick"]["line"]
            .as_str()
            .unwrap()
            .contains("settling #391 once its worker leaves")
    );
    let findings = doctor["findings"].as_array().unwrap();
    assert!(
        !findings.iter().any(|finding| matches!(
            finding["kind"].as_str(),
            Some("step_unrecovered" | "settle_waiting")
        )),
        "{findings:#?}"
    );
    let all =
        world.afk(&["doctor", "--all", "--host", "h1", "--since", "1000", "--now", "1090"]).json;
    let findings = all["checkouts"][0]["doctor"]["findings"].as_array().unwrap();
    assert!(!findings.iter().any(|finding| finding["kind"] == "tick_failed"), "{findings:#?}");
    assert!(!session.ended(), "a worker still working is left alone");
    assert_eq!(open_terminals(&world, &worktree).len(), 1);
    assert!(worktree.is_dir());
    assert_eq!(world.claims(), alpha(&[391]));

    report(&world, "idle", 1100);
    let (lines, _) = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        "1180",
        "--ticks",
        "1",
        "--second-ms",
        "0",
    ]);
    // A new run has no previous tick to attribute an outside closure to.
    assert!(lines.iter().any(|line| line.contains("settled #391")), "{lines:?}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1180"]).json;
    assert!(doctor["fleet"]["last_tick"]["line"].as_str().unwrap().contains("settled #391"));
    assert!(session.ended());
    assert!(!worktree.exists(), "once it stopped, the next tick ends it and removes the worktree");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_run_says_left_after_a_finishing_worker_leaves() {
    let world = backlog(&[391]);
    let running =
        world.running(&["--instance", "alpha", "--host", "h1", "--ticks", "3", "--now", "1000"]);
    let soon = std::time::Duration::from_secs(20);
    assert!(running.line(soon).unwrap().contains("dispatched #391"));
    let worktree = worker_worktree(&world);
    let session = Session::start(&world, &worktree);
    report(&world, "working", 1000);
    world.close_issue(391);
    world.afk(&["wake"]).ok();
    let waiting = running.line(soon).unwrap();
    assert!(waiting.contains("settling #391 once its worker leaves"), "{waiting}");
    assert!(!waiting.contains("FAILED"), "{waiting}");
    report(&world, "idle", 1000);
    world.afk(&["wake"]).ok();
    let left = running.line(soon).unwrap();
    assert!(left.contains("left #391"), "{left}");
    assert!(session.ended());
    assert!(!worktree.exists());
}

#[test]
fn a_worker_still_busy_after_the_settle_grace_gets_exactly_one_finding() {
    let world = backlog(&[381]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    let _session = Session::start(&world, &worktree);
    report(&world, "working", 1000);
    world.close_issue(381);

    tick(&world, "1090");
    let waiting = tick(&world, "1450");
    assert_eq!(waiting["errors"], json!([]), "{waiting}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1450"]).json;
    let found: Vec<_> = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["kind"] == "settle_waiting")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0]["what"].as_str().unwrap().contains("#381"), "{found:#?}");
    assert!(found[0]["what"].as_str().unwrap().contains(worktree.to_str().unwrap()));
    let evidence = found[0]["evidence"].as_str().unwrap();
    assert!(evidence.contains("busy"), "{evidence}");
    assert!(evidence.contains("processes"), "{evidence}");
    assert!(evidence.contains("tick-"), "{evidence}");
}

#[test]
fn a_worktree_with_a_commit_the_remote_does_not_have_stays() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    std::fs::write(worktree.join("only-here.txt"), "local").unwrap();
    git(&worktree, &["add", "only-here.txt"]);
    git(&worktree, &["commit", "-q", "-m", "only here"]);
    world.close_issue(3);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done"], "{ticked}");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    assert!(worktree.is_dir(), "a commit the remote does not have stays");
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(common::worktree_listed(&listed, &worktree), "{listed}");
    assert_no_removal_error(&world);
    assert_eq!(world.claims(), []);

    tick(&world, "1180");
    assert!(worktree.is_dir(), "a later tick does not take it");
}

#[test]
fn a_left_removal_with_a_commit_the_remote_does_not_have_stays() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let worktree = worker_worktree(&world);
    std::fs::write(worktree.join("only-here.txt"), "local").unwrap();
    git(&worktree, &["add", "only-here.txt"]);
    git(&worktree, &["commit", "-q", "-m", "only here"]);
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    seal(&worktree);
    unregister(&world, &worktree);
    record_failed_removal(&world, &worktree, &head);
    world.close_issue(3);
    world.remote(&["update-ref", "-d", "refs/convoy/claim/3"]);

    let ticked = tick(&world, "1090");
    assert!(ticked["errors"].as_array().unwrap().is_empty(), "{ticked}");
    assert!(worktree.is_dir(), "a leftover that holds an unpushed commit stays");
    let orca = world.orca();
    let row = orca["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == worktree.to_str().unwrap())
        .unwrap();
    assert!(!row["removalError"].as_str().unwrap().is_empty());
}

fn worker_worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let row = orca["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["isMainWorktree"] != true && row["linkedIssue"].as_u64().is_some())
        .unwrap();
    PathBuf::from(row["path"].as_str().unwrap())
}

/// A directory the owner cannot write, with a file in it: an empty one does
/// not stop `git worktree remove`.
fn seal(worktree: &Path) -> PathBuf {
    let dir = worktree.join("cache/afkconvoy/copies/role");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "role").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    dir
}

fn unregister(world: &World, path: &Path) {
    let out = Command::new("git")
        .current_dir(&world.clone)
        .args(["worktree", "remove", "--force"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "git removed the worktree despite a sealed directory: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(path.is_dir(), "the directory stays");
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(!common::worktree_listed(&listed, path), "{listed}");
}

fn record_failed_removal(world: &World, path: &Path, head: &str) {
    let text = path.to_string_lossy().into_owned();
    world.with_orca(|orca| {
        let row = orca["worktrees"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["path"] == text)
            .unwrap();
        row["removalError"] =
            json!(format!("Failed to force delete worktree at {text}. Permission denied"));
        row["head"] = json!(head);
    });
}

fn assert_no_removal_error(world: &World) {
    for row in world.orca()["worktrees"].as_array().unwrap() {
        let error = row["removalError"].as_str().unwrap_or("");
        assert!(error.is_empty(), "{row}");
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Kills the process when dropped, including when an assertion fails.
/// The workers report `state` since `since` (epoch seconds).
fn report(world: &World, state: &str, since: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": state, "stateStartedAt": since * 1000});
            terminal["lastOutputAt"] = json!(since * 1000);
        }
    });
}

fn open_terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).cloned().collect()
}

/// A process of the worker's own session: started in its worktree with its
/// `TMPDIR`, as the harness orca starts for it is. A thread reaps it the
/// moment it ends, so no zombie stands for it.
#[test]
#[ignore = "a worker's session starts this as its process"]
fn session_helper() {
    std::fs::write(std::env::var_os("SESSION_READY").unwrap(), "").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(120));
}

pub(crate) struct Session {
    pid: u32,
    reaper: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    pub(crate) fn start(world: &World, worktree: &Path) -> Self {
        // Not a system binary: `ps` reads the environment of this one.
        let ready = worktree.join(".afkconvoy-tmp/session-ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tick::session_helper", "--ignored"])
            .current_dir(worktree)
            .env("TMPDIR", world.tmp(worktree))
            .env("SESSION_READY", &ready)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let reaper = std::thread::spawn(move || {
            child.wait().unwrap();
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.exists() {
            assert!(std::time::Instant::now() < until, "the session never started");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Self { pid, reaper: Some(reaper) }
    }

    fn ended(&self) -> bool {
        self.reaper.as_ref().is_none_or(|reaper| reaper.is_finished())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(reaper) = self.reaper.take() {
            if !reaper.is_finished() {
                // SAFETY: this pid is the test's own child, not yet reaped.
                unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
            }
            reaper.join().unwrap();
        }
    }
}

struct Hold(Child);

impl Drop for Hold {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_start_that_failed_holds_its_claim_and_its_slot() {
    let world = backlog(&[3, 5, 9]);
    world.with_orca(|orca| orca["create_fails"] = json!([3]));
    let ticked = tick(&world, "1000");
    assert_eq!(
        did(&ticked),
        ["dispatch 3 failed", "dispatch 5 done", "dispatch 9 done", "heartbeat done"]
    );
    assert_eq!(ticked["held"], 3);
    assert_eq!(world.claims(), alpha(&[3, 5, 9]));
}

#[test]
fn a_worker_that_never_took_its_brief_is_started_again_in_the_same_worktree() {
    for knob in ["never_ready", "send_refused"] {
        let world = backlog(&[3]);
        world.with_orca(|orca| orca[knob] = json!(true));
        let failed = tick(&world, "1000");
        assert_eq!(did(&failed), ["dispatch 3 failed", "heartbeat done"]);
        let before = world.orca();
        let path = before["worktrees"][0]["path"].clone();
        let branch = before["worktrees"][0]["branch"].clone();
        let path_dir = std::path::Path::new(path.as_str().unwrap());
        std::fs::write(path_dir.join("scratch.txt"), "keep uncommitted progress").unwrap();
        world.with_orca(|orca| {
            orca[knob] = json!(false);
            orca["trust_prompt"] = json!(false);
        });
        world.orca_calls();

        let again = tick(&world, "1090");
        assert!(again["errors"].as_array().unwrap().is_empty(), "{again}");
        let calls = world.orca_calls();
        assert!(
            calls.iter().any(|call| common::asked(call) == "terminal create"),
            "{knob}: the next tick must start the worker again: {again}"
        );
        assert!(!calls.iter().any(|call| common::asked(call) == "worktree create"));
        let after = world.orca();
        assert_eq!(after["worktrees"][0]["path"], path);
        assert_eq!(after["worktrees"][0]["branch"], branch);
        assert_eq!(
            std::fs::read_to_string(path_dir.join("scratch.txt")).unwrap(),
            "keep uncommitted progress"
        );
        let terminals = after["terminals"].as_array().unwrap();
        let worker = terminals.iter().find(|terminal| terminal["open"] == true).unwrap();
        let sent = worker["sent"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0]["text"].as_str().unwrap().contains("convoy-worker-prompt.md"));
        assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
        assert_eq!(world.claims(), alpha(&[3]));
        // A successful delivery ended startup: the next tick leaves this worker alone.
        world.orca_calls();
        tick(&world, "1180");
        assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"));
    }
}

#[test]
fn a_worktree_trust_gap_is_reported_once_not_as_a_failed_dispatch_every_tick() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["trust_prompt"] = json!(true));
    let first = tick(&world, "1000");
    assert!(first["errors"][0]["error"].as_str().unwrap().contains("worktree trust gap"));
    assert!(world.labels(3).contains(&"ready-for-human".to_string()));
    assert_eq!(world.claims(), []);
    let comments = world.comments(3);
    world.orca_calls();
    let again = tick(&world, "1090");
    assert_eq!(again["errors"], json!([]), "{again}");
    assert!(!again.to_string().contains("dispatch 3"), "{again}");
    assert_eq!(world.comments(3), comments);
    let doctor = world.afk(&["doctor", "--now", "1100"]).json;
    let why = doctor["waiting_on_human"][0]["why"].as_str().unwrap();
    assert!(why.contains("Fleet fault:") && why.contains("the fix is on the host"), "{doctor}");
    let calls = world.orca_calls();
    assert!(
        !calls.iter().any(|call| common::asked(call) == "terminal create"),
        "no repeated start: {calls:?}"
    );
}

#[test]
fn a_start_waits_for_its_predecessors_terminal_to_finish_closing() {
    let world = backlog(&[3]);
    // Orca acknowledges the close, and the worktree's shell is still there.
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    for at in ["1000", "1090", "1180"] {
        let waiting = tick(&world, at);
        assert_eq!(
            waiting["errors"],
            json!([]),
            "a terminal still closing fails no tick: {waiting}"
        );
        assert_eq!(world.claims(), alpha(&[3]));
        assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
        assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"));
    }
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(false));
    let started = tick(&world, "1270");
    assert_eq!(started["errors"], json!([]), "{started}");
    assert!(
        world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"),
        "the replacement starts on the tick after the close"
    );
    assert_eq!(world.claims(), alpha(&[3]));
    assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
    let worker = world.open_terminals();
    assert_eq!(worker.len(), 1, "{worker:?}");
}

#[test]
fn a_predecessors_terminal_still_there_past_the_bound_escalates_saying_how_long() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["close_keeps_terminal"] = json!(true));
    for at in ["1000", "1150"] {
        assert_eq!(tick(&world, at)["errors"], json!([]));
        assert_eq!(world.claims(), alpha(&[3]));
    }
    let failed = tick(&world, "1300");
    let error = failed["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("waited 300s"), "{failed}");
    assert_eq!(world.claims(), []);
    assert!(world.labels(3).contains(&"ready-for-human".to_string()));
    let comments = world.comments(3);
    let escalation = comments.iter().find(|body| body.contains("convoy:escalation")).unwrap();
    assert!(escalation.contains("worker start failed again"), "{escalation}");
    assert!(escalation.contains("still has a terminal after close"), "{escalation}");
    assert!(escalation.contains("waited for 300s"), "{escalation}");
    assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"));
}

#[test]
fn a_worker_start_that_fails_again_escalates_with_the_start_errors() {
    for knob in ["create_fails", "never_ready", "send_refused"] {
        let world = backlog(&[3]);
        world.with_orca(|orca| {
            orca[knob] = if knob == "create_fails" { json!([3]) } else { json!(true) };
        });
        let first = tick(&world, "1000");
        let error = first["errors"][0]["error"].as_str().unwrap();
        assert_eq!(world.claims(), alpha(&[3]));
        let again = tick(&world, "1090");
        assert_eq!(world.claims(), [], "{knob}: {again}");
        assert!(world.labels(3).contains(&"ready-for-human".to_string()));
        let comments = world.comments(3);
        let escalation = comments.iter().find(|body| body.contains("convoy:escalation")).unwrap();
        assert!(escalation.contains("worker start failed again"), "{escalation}");
        assert!(escalation.contains(error), "the exact original error: {escalation}");
        assert!(escalation.contains("Failed start:"), "{escalation}");
        assert!(!escalation.contains("went silent"), "{escalation}");
        assert!(
            escalation.contains("Fleet fault:") && escalation.contains("the fix is on the host")
        );
        assert!(world.open_terminals().is_empty());
        for terminal in world.orca()["terminals"].as_array().unwrap() {
            assert_eq!(terminal["sent"], json!([]), "no nudge or brief reached this shell");
        }
        tick(&world, "1600");
        assert_eq!(world.claims(), []);
        assert!(!world.comments(3).iter().any(|body| body.contains("convoy:consult")));
        let doctor = world.afk(&["doctor", "--now", "1610"]).json;
        let why = doctor["waiting_on_human"][0]["why"].as_str().unwrap();
        assert!(why.contains("Fleet fault:") && why.contains("the fix is on the host"), "{doctor}");
    }
}

#[test]
fn an_incomplete_dispatch_does_not_belong_to_a_later_claim() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["never_ready"] = json!(true));
    tick(&world, "1000");
    world.afk(&["release", "--issue", "3", "--instance", "alpha"]).ok();
    let new = tick(&world, "1090");
    assert_eq!(did(&new), ["dispatch 3 failed", "heartbeat done"]);
    assert_eq!(world.claims(), alpha(&[3]));
    assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
}

#[test]
fn an_issue_a_peer_holds_is_passed_over() {
    let world = backlog(&[3, 5, 9]);
    world.afk(&["claim", "--issue", "3", "--instance", "beta", "--now", "1000"]).ok();
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 5 done", "dispatch 9 done", "heartbeat done"]);
    assert_eq!(ticked["held"], 2);
}

#[test]
fn a_tick_holding_nothing_writes_nothing() {
    let world = World::new();
    world.watch_pushes(None);
    let ticked = tick(&world, "1000");
    assert_eq!(ticked["did"], json!([]));
    assert_eq!(world.pushes(), [] as [String; 0]);
}

#[test]
fn a_tick_leaves_nothing_behind_on_the_host() {
    let world = backlog(&[3]);
    let refs_before = git(&world.clone, &["for-each-ref"]);
    tick(&world, "1000");
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "");
    let refs_after = git(&world.clone, &["for-each-ref"]);
    let new: Vec<&str> = refs_after.lines().filter(|line| !refs_before.contains(line)).collect();
    assert_eq!(new.len(), 1, "only the worker's branch: {new:?}");
    assert!(new[0].contains("refs/heads/tester/issue-3"));
}

fn run(world: &World, extra: &[&str]) -> (Vec<String>, Value) {
    let mut args = vec!["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    args.extend(extra);
    world.afk_lines(&args)
}

#[test]
fn run_ticks_until_stopped_and_says_what_each_tick_did() {
    let world = backlog(&[3, 5, 9]);
    let (lines, stopped) = run(&world, &["--instance", "alpha", "--ticks", "2", "--now", "1000"]);
    assert_eq!(
        lines,
        [
            "tick 1: dispatched #3, #5, #9 · holding 3 · frontier 0 · next in 90s",
            // their workers, started a moment ago, are left a grace period
            "tick 2: nothing to do · holding 3 · frontier 0 · next in 90s",
        ]
    );
    assert_eq!(stopped["ticks"], 2);
    // stopping is exiting: nothing is released on the way out
    assert_eq!(world.claims(), alpha(&[3, 5, 9]));
}

#[test]
fn a_tick_says_which_issue_it_skipped_for_a_blocker_read_after_its_rebuild() {
    let world = backlog(&[3, 5]);
    // the rebuild reads no blocker of #3: GitHub records it as it answers the dispatch
    world.with_gh(|gh| {
        gh["edges"]["3"] = json!([5]);
        gh["issues"][0]["blockers_at_rebuild"] = json!([]);
    });
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    assert_eq!(
        lines,
        [
            "tick 1: dispatched #5 · skipped #3 (blocked by #5) · holding 1 · frontier 0 · next in 90s"
        ]
    );
    assert_eq!(world.claims(), alpha(&[5]));
}

#[test]
fn a_frontier_issue_closed_after_its_rebuild_is_skipped_and_fails_nothing() {
    let world = backlog(&[3, 5]);
    // the rebuild lists #3 open; it closed before its dispatch read it
    world.close_issue(3);
    world.with_gh(|gh| {
        let row = gh["issues"].as_array_mut().unwrap().iter_mut().find(|i| i["number"] == 3);
        row.unwrap()["open_at_rebuild"] = json!(true);
    });
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 3 closed", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(ticked["errors"], json!([]), "no failed step to raise a finding on");
    assert_eq!(world.claims(), alpha(&[5]));
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    assert!(lines[0].contains("#3 closed meanwhile"), "{lines:?}");
    assert!(!lines[0].contains("FAILED"), "{lines:?}");
}

#[test]
fn run_goes_idle_after_three_empty_ticks() {
    let world = World::new();
    world.list_checkout(&world.clone);
    let (lines, _) = run(&world, &["--ticks", "4"]);
    let sleeps: Vec<&str> =
        lines.iter().map(|line| line.rsplit("next in ").next().unwrap()).collect();
    assert_eq!(sleeps, ["90s", "90s", "1500s", "1500s"]);
    assert!(lines[0].starts_with("tick 1: nothing to do · holding 0 · frontier 0"));
}

#[test]
fn run_follows_a_tick_that_settled_a_claim_with_no_sleep_and_sleeps_after_one_that_did_nothing() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    world.close_issue(3);
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "2", "--now", "1090"]);
    assert_eq!(
        lines,
        [
            // what the landing unblocked is the next tick's to find
            "tick 1: settled #3 · holding 0 · frontier 0 · next now",
            "tick 2: nothing to do · holding 0 · frontier 0 · next in 90s",
        ]
    );
}

#[test]
fn every_tick_says_as_it_starts_why_it_started() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    world.close_issue(3);
    let args = ["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    let more = ["--instance", "alpha", "--ticks", "3", "--now", "1090"];
    let (lines, _) = world.afk_every_line(&[&args[..], &more[..]].concat());
    // The host's watch has its own log events, independent of this fleet's ticks.
    let lines: Vec<_> = lines.into_iter().filter(|line| !line.starts_with("watch: ")).collect();
    assert_eq!(
        lines,
        [
            "start: run as alpha on h1, its workers started with `agent`",
            "tick 1: started (start)",
            "tick 1: settled #3 · holding 0 · frontier 0 · next now",
            "tick 2: started (follow-up)",
            "tick 2: nothing to do · holding 0 · frontier 0 · next in 90s",
            "tick 3: started (timer)",
            "tick 3: nothing to do · holding 0 · frontier 0 · next in 90s",
        ]
    );
}

/// A fleet that worked and then went idle never puts its host to sleep: it
/// asks `pmset` nothing, however long it stays idle (docs/adr/0050).
#[test]
fn a_fleet_that_worked_and_went_idle_never_puts_its_host_to_sleep() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let claim = ["claim", "--issue", "7", "--instance", "alpha", "--host", "h1"];
    assert_eq!(world.afk(&claim).ok()["won"], true);
    world.close_issue(7);
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1", "--second-ms", "0"];
    let (lines, _) = world.afk_every_line(&[&args[..], &["--ticks", "3"]].concat());
    assert!(lines.iter().any(|line| line.starts_with("tick 3: nothing to do")), "{lines:#?}");
    assert!(!lines.iter().any(|line| line.contains("host sleep")), "{lines:#?}");
    assert_eq!(world.host_calls("pmset"), Vec::<String>::new());
}

#[test]
fn run_mints_an_instance_id_named_for_the_host() {
    let world = backlog(&[3]);
    let (_, stopped) = run(&world, &["--ticks", "1"]);
    let instance = stopped["instance"].as_str().unwrap();
    assert!(instance.starts_with("h1-") && instance.len() == "h1-".len() + 8, "{instance}");
    assert_eq!(world.claims(), [(3, instance.to_string())]);
}

#[test]
fn a_tick_that_cannot_rebuild_is_a_line_and_the_loop_goes_on() {
    let world = backlog(&[3]);
    world.with_gh(|gh| gh["fail"] = json!(["pr list"]));
    let (lines, stopped) = run(&world, &["--ticks", "2"]);
    assert!(lines[0].starts_with("tick 1: FAILED: gh pr list failed"), "{lines:?}");
    assert!(lines[1].starts_with("tick 2: FAILED"));
    assert_eq!(stopped["ticks"], 2);
}

#[test]
fn a_failed_step_shows_in_the_line() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["create_fails"] = json!([3]));
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1"]);
    assert!(lines[0].contains("FAILED dispatch #3: orca worktree create failed"), "{lines:?}");
    // every step it had failed: that is not having had nothing to do
    assert!(!lines[0].contains("nothing to do"), "{lines:?}");
    assert!(lines[0].starts_with("tick 1: holding 1 · frontier 0 · FAILED"), "{lines:?}");
}

/// Takes issue `number` off the stand-in GitHub as `how` — `deleted`,
/// `transferred` or `missing` — once `calls` more calls about it are answered.
fn gone(world: &World, number: u64, how: &str, calls: u64) {
    world.with_gh(|gh| {
        if calls == 0 {
            gh["issues"].as_array_mut().unwrap().retain(|issue| issue["number"] != number);
        }
        gh[how] = json!({ number.to_string(): calls });
    });
}

#[test]
fn a_claimed_issue_deleted_between_two_ticks_is_let_go_and_its_slot_refilled() {
    for (how, said) in [
        ("deleted", "the issue was deleted"),
        ("transferred", "the issue was transferred to acme/elsewhere#1003"),
        ("missing", "there is no such issue"),
    ] {
        let world = backlog(&[3, 5]);
        tick(&world, "1000");
        world.issue(9, &["ready-for-agent"]);
        let worktree = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
        gone(&world, 3, how, 0);

        let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1090"]);
        assert_eq!(
            lines,
            [format!(
                "tick 1: released #3 ({said}) · dispatched #9 · holding 2 · frontier 0 · \
                 next in 90s"
            )],
            "{how}"
        );
        assert_eq!(world.claims(), alpha(&[5, 9]), "{how}");
        // it held nothing the remote does not
        assert!(!std::path::Path::new(&worktree).exists(), "{how}: the worktree is removed");
        // nothing is written on an issue that is gone
        assert_eq!(world.boards(3).len(), 1, "{how}: only the board of its dispatch");
    }
}

#[test]
fn a_tick_lets_go_of_a_deleted_issue_in_its_answer_with_no_error() {
    let world = backlog(&[3, 5]);
    tick(&world, "1000");
    gone(&world, 3, "deleted", 0);
    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["forget 3 done", "heartbeat done"]);
    assert_eq!(ticked["errors"], json!([]));
}

#[test]
fn an_issue_deleted_during_its_own_dispatch_leaves_no_claim_behind() {
    let world = backlog(&[3, 5]);
    // answered as the dispatch reads it and its blockers; deleted once it is claimed
    gone(&world, 3, "deleted", 2);
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    assert_eq!(
        lines,
        ["tick 1: released #3 (the issue was deleted) · dispatched #5 · holding 1 · frontier \
             0 · next in 90s"]
    );
    assert_eq!(world.claims(), alpha(&[5]));
}

#[test]
fn an_issue_deleted_before_its_dispatch_claims_nothing_and_takes_no_slot() {
    let world = backlog(&[3, 5, 9]);
    gone(&world, 3, "deleted", 0);
    // the rebuild's list still had it
    world.with_gh(|gh| {
        gh["issues"].as_array_mut().unwrap().push(json!({"number": 3,
        "title": "Issue 3", "labels": ["ready-for-agent"], "state": "open"}))
    });
    let ticked = tick(&world, "1000");
    assert_eq!(
        did(&ticked),
        ["dispatch 3 gone", "dispatch 5 done", "dispatch 9 done", "heartbeat done"]
    );
    assert_eq!(world.claims(), alpha(&[5, 9]));
}

#[test]
fn a_claimed_issue_the_rebuild_cannot_read_is_an_error_of_that_tick_alone() {
    let world = backlog(&[3, 5]);
    tick(&world, "1000");
    world.issue(9, &["ready-for-agent"]);
    world.with_gh(|gh| {
        gh["issues"].as_array_mut().unwrap().retain(|issue| issue["number"] != 3);
        gh["fail"] = json!(["issues/3 --jq"]);
    });
    world.close_issue(5);
    let ticked = tick(&world, "1090");
    assert_eq!(ticked["errors"][0]["do"], "read");
    assert_eq!(ticked["errors"][0]["issue"], 3);
    assert!(ticked["errors"][0]["error"].as_str().unwrap().contains("injected failure"));
    // the rest of the tick is still made
    assert_eq!(did(&ticked)[..2], ["settle 5 done", "dispatch 9 done"]);
    assert_eq!(world.claims(), alpha(&[3, 9]));
}

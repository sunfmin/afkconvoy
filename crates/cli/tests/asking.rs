//! Human decisions from watch reports are asked in one resident Terminal,
//! once per condition episode, with recovery and input readiness enforced.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::common::{FixtureProcess, World, asked};

fn operator(world: &World) -> PathBuf {
    let dir = world.root.join("operator");
    std::fs::create_dir_all(&dir).unwrap();
    world.with_orca(|orca| {
        orca["worktrees"].as_array_mut().unwrap().push(
            json!({"path": dir, "repoId": "operator", "isMainWorktree": true, "isArchived": false}),
        );
        orca["terminals"].as_array_mut().unwrap().push(
            json!({"handle": "operators-own", "worktreePath": dir, "command": "claude", "sent": [],
                "open": true}),
        );
    });
    dir
}

fn run(world: &World, dir: &Path) -> FixtureProcess {
    world.say_host_settings(json!({"watch_dir": dir, "watch_every": 1}));
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root);
    command.args(["host-runner", "--kept-by", "0"]);
    let runner = FixtureProcess::spawn(&mut command);
    until(world, "owned host runner", || {
        (world.host_runner() == Some(runner.id())
            && world.cache().join("afkconvoy/host-runner.sock").exists())
        .then_some(())
    });
    // The lock and listening socket precede panel initialization and the
    // request-serving thread. A restart must wait for a response, not just
    // those files, before `run` makes its bounded version request.
    until(world, "host runner answering requests", || {
        let answer = world.afk(&["checkouts"]);
        (answer.code == 0 && answer.json["host_runner"]["pid"] == runner.id()).then_some(())
    });
    world
        .afk(&[
            "run",
            "--host",
            "h1",
            "--second-ms",
            "100",
            "--watch-dir",
            dir.to_str().unwrap(),
            "--watch-every",
            "1",
        ])
        .ok();
    runner
}

fn until<T>(world: &World, what: &str, mut got: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(found) = got() {
            return found;
        }
        assert!(Instant::now() < until, "no {what} within 60s: {}", asking_state(world));
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn paused(world: &World, harness: &str, why: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    world.pause(harness, now.as_secs(), why);
}

fn unpaused(world: &World, harness: &str) {
    let mut paused = world.paused();
    paused.as_array_mut().unwrap().retain(|row| row["harness"] != harness);
    std::fs::write(world.paused_path(), paused.to_string()).unwrap();
}

fn watch_state(world: &World) -> Value {
    std::fs::read(world.cache().join("afkconvoy/watch/state.json"))
        .map_or(json!({}), |bytes| serde_json::from_slice(&bytes).unwrap())
}

fn asking_state(world: &World) -> Value {
    std::fs::read(world.cache().join("afkconvoy/asking/state.json"))
        .map_or(json!({}), |bytes| serde_json::from_slice(&bytes).unwrap())
}

fn watch_running(world: &World) -> Option<Value> {
    Some(watch_state(world)["running"].clone()).filter(|running| running.is_object())
}

fn report_humans(world: &World, watch: &Value, human: Value) {
    let file = world.root.join(format!("human-report-{}.json", watch["id"].as_str().unwrap()));
    std::fs::write(
        &file,
        json!({"held_back": [], "did": [], "verified": [], "human": human, "pending": []})
            .to_string(),
    )
    .unwrap();
    world
        .afk(&[
            "watched",
            "--watch",
            watch["id"].as_str().unwrap(),
            "--text-file",
            file.to_str().unwrap(),
        ])
        .ok();
    until(world, "watch report", || {
        let last = watch_state(world)["last"].clone();
        (last["id"] == watch["id"] && last["outcome"] == "ended").then_some(())
    });
}

fn two_conditions(world: &World) -> (PathBuf, FixtureProcess) {
    let dir = operator(world);
    paused(world, "pi", "an account is needed");
    world.issue(11, &["ready-for-human"]);
    world.comment(11, "<!--convoy:escalation claim=old-->\nNeeds a quiet measurement environment.");
    let runner = run(world, &dir);
    (dir, runner)
}

fn conditions(watch: &Value) -> (String, String) {
    let ids = watch["conditions"].as_object().unwrap();
    let wait = ids.keys().find(|id| id.contains("\"human\"")).unwrap().clone();
    let host = ids.keys().find(|id| !id.contains("\"human\"")).unwrap().clone();
    (wait, host)
}

fn asking_terminal(world: &World) -> Value {
    let handle = asking_state(world)["session"]["terminal"].as_str().unwrap().to_string();
    world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["handle"] == handle)
        .unwrap()
        .clone()
}

fn asking_brief(world: &World) -> String {
    let terminal = asking_terminal(world);
    let line = terminal["sent"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap();
    let marker = "Your task brief is the file ";
    let at = line.find(marker).unwrap() + marker.len();
    let file = &line[at..line[at..].find(" — ").unwrap() + at];
    std::fs::read_to_string(file).unwrap()
}

fn push_text(brief: &str) -> &str {
    let at = brief.find("```text\n").unwrap() + "```text\n".len();
    &brief[at..brief[at..].find("\n```").unwrap() + at]
}

fn settle(world: &World, outcome: &str) {
    let current = asking_state(world)["current"].clone();
    let id = current["question"]["id"].as_str().unwrap();
    let file = world.root.join(format!("asked-{id}.json"));
    let mut report = json!({"outcome": outcome});
    if outcome == "settled" {
        report["url"] = json!("https://gh/c/operator-decision");
    }
    std::fs::write(&file, report.to_string()).unwrap();
    world.afk(&["asked", "--question", id, "--text-file", file.to_str().unwrap()]).ok();
}

fn asking_creates(world: &World) -> usize {
    world
        .orca_called_with()
        .into_iter()
        .filter(|(call, _)| {
            asked(call) == "terminal create"
                && call.windows(2).any(|args| args == ["--title", "afkconvoy asks"])
        })
        .count()
}

fn takeover_fences_outbox(notify: bool) {
    let world = World::new();
    let (dir, runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (wait, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([
            {"condition": host, "question": "Host's own question?", "answer": "host answer"},
            {"condition": wait, "question": "Taken issue question?", "answer": "issue answer"}
        ]),
    );
    until(&world, "first question delivered", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    let terminal = asking_state(&world)["session"]["terminal"].as_str().unwrap().to_string();
    world.with_orca(|orca| {
        for row in orca["terminals"].as_array_mut().unwrap() {
            if row["handle"] == terminal {
                row["tui_busy"] = json!(true);
            }
        }
    });
    settle(&world, "settled");
    until(&world, "issue queued behind busy input", || {
        (asking_state(&world)["current"]["question"]["condition"] == wait
            && asking_state(&world)["current"]["delivered"] == false)
            .then_some(())
    });
    let host_name = world.checkout_list()["settings"]["host_name"].clone();
    drop(runner);
    // A takeover happens while this host is down, after its report and
    // asking outboxes have been persisted. No new look will reconcile them.
    let repo = afkconvoy::git::Repo::at(&world.clone).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let sha = repo.record_commit(&format!("convoy:claim instance=peer host=h2 ts={now}")).unwrap();
    crate::common::git(
        &world.clone,
        &["push", "-q", "--force", "origin", &format!("{sha}:refs/convoy/wait/11")],
    );
    let output = world.root.join("notifications.jsonl");
    let mut settings = json!({"host_name": host_name, "watch_dir": dir, "watch_every": 3600});
    if notify {
        settings["notify_command"] = json!(format!("cat >> {}", output.display()));
    }
    world.say_host_settings(settings);
    if !notify {
        world.with_orca(|orca| {
            for row in orca["terminals"].as_array_mut().unwrap() {
                if row["handle"] == terminal {
                    row["tui_busy"] = json!(false);
                }
            }
        });
    }
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root);
    command.args(["host-runner", "--kept-by", "0"]);
    let _restarted = FixtureProcess::spawn(&mut command);
    until(&world, "taken wait dropped without another look", || {
        let path = world.cache().join("afkconvoy/watch/notify.json");
        let outbox: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        outbox["entries"].as_object()?.values().all(|e| e["condition"] != wait).then_some(())
    });
    until(&world, "asking queue drops taken wait", || {
        asking_state(&world)["current"].is_null().then_some(())
    });
    assert_eq!(
        asking_terminal(&world)["sent"].as_array().unwrap().len(),
        1,
        "the second question never goes out"
    );
    if notify {
        until(&world, "host condition still sends", || output.exists().then_some(()));
        let lines = std::fs::read_to_string(&output).unwrap();
        let sent: Vec<Value> = lines.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["condition"], host);
    } else {
        assert!(!output.exists());
    }
    assert!(world.runner_log().join("\n").contains("held by h2"));
}

#[test]
fn an_asking_question_rechecks_ownership_after_restart_and_drops_a_taken_wait() {
    takeover_fences_outbox(false);
}

#[test]
fn a_notification_rechecks_ownership_after_restart_and_keeps_host_conditions_local() {
    takeover_fences_outbox(true);
}

#[test]
fn two_conditions_share_one_session_and_are_delivered_one_at_a_time() {
    let world = World::new();
    let (dir, runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (wait, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([
            {"condition": wait, "question": "Provide the quiet environment?", "answer": "afkconvoy rule --issue 11 --text-file <file>"},
            {"condition": host, "question": "Approve the account spend?", "answer": "afkconvoy kit"}
        ]),
    );
    until(&world, "first delivered question", || {
        let current = asking_state(&world)["current"].clone();
        (current["delivered"] == true).then_some(current)
    });
    let first = asking_state(&world);
    assert_eq!(first["current"]["question"]["human"]["question"], "Provide the quiet environment?");
    assert_eq!(first["pending"].as_array().unwrap().len(), 1);
    assert_eq!(asking_creates(&world), 1);
    let terminal = asking_terminal(&world);
    assert_eq!(terminal["title"], "afkconvoy asks");
    assert_eq!(terminal["worktreePath"], json!(dir));
    assert_eq!(terminal["sent"].as_array().unwrap().len(), 1);
    assert!(!terminal["command"].as_str().unwrap().contains("--terminal-input-only"));
    let orca = world.orca();
    let watch_terminal = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["title"] == "afkconvoy watch")
        .unwrap();
    assert!(watch_terminal["command"].as_str().unwrap().contains("--terminal-input-only"));
    let brief = asking_brief(&world);
    assert!(brief.contains(first["current"]["question"]["condition"].as_str().unwrap()));
    assert!(brief.contains("https://gh/c/2000"));
    assert!(brief.contains("1 more questions wait"));
    let push = push_text(&brief);
    assert!(push.starts_with("Provide the quiet environment?"), "{push}");
    assert!(push.chars().count() <= 180);

    settle(&world, "settled");
    until(&world, "second delivered question", || {
        let current = asking_state(&world)["current"].clone();
        (current["question"]["human"]["question"] == "Approve the account spend?"
            && current["delivered"] == true)
            .then_some(())
    });
    assert_eq!(asking_creates(&world), 1);
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 2);
    assert!(push_text(&asking_brief(&world)).starts_with("Approve the account spend?"));
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["asking"]["status"], "asking");
    assert_eq!(
        doctor["host"]["asking"]["current"]["question"]["human"]["question"],
        "Approve the account spend?"
    );
    assert_eq!(doctor["host"]["asking"]["pending"], json!([]));
    let panel =
        world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone).arg("panel").output().unwrap();
    let panel = String::from_utf8(panel.stdout).unwrap();
    assert!(panel.contains("data-asking=\"session\""), "{panel}");
    assert!(panel.contains("Approve the account spend?"), "{panel}");
    for args in [["run", "--help"].as_slice(), ["help", "humans"].as_slice()] {
        let help = world
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
            .args(args)
            .output()
            .unwrap();
        let help = String::from_utf8(help.stdout).unwrap();
        assert!(help.contains("afkconvoy asks"), "{help}");
        assert!(help.contains("180"), "{help}");
    }
    settle(&world, "settled");
    until(&world, "resident standby after every question settled", || {
        (asking_state(&world)["current"].is_null()
            && world.afk(&["doctor"]).json["host"]["asking"]["status"] == "standby")
            .then_some(())
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 2);
    assert_eq!(asking_creates(&world), 1);
    drop(runner);
    assert!(world.host_runner().is_none());
}

#[test]
fn reworded_reports_and_runner_restart_do_not_ask_an_episode_twice() {
    let world = World::new();
    let (_dir, runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (_, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([{"condition": host, "question": "Supply an account?", "answer": "afkconvoy kit"}]),
    );
    until(&world, "delivered question", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    let episode = asking_state(&world)["current"]["question"]["id"].as_str().unwrap().to_string();
    drop(runner);
    let _runner = run(&world, &world.root.join("operator"));
    until(&world, "recovered asking session", || {
        (asking_state(&world)["current"]["question"]["id"] == json!(episode)
            && asking_state(&world)["current"]["delivered"] == true)
            .then_some(())
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(asking_creates(&world), 1);
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 1);
    settle(&world, "settled");
    until(&world, "settled question", || asking_state(&world)["current"].is_null().then_some(()));

    paused(&world, "kimi", "an unrelated new fault");
    let next = until(&world, "another watch round", || {
        watch_running(&world).filter(|next| next["id"] != watch["id"])
    });
    assert!(next["conditions"].as_object().unwrap().contains_key(&host));
    report_humans(
        &world,
        &next,
        json!([{"condition": host, "question": "The same account, reworded?", "answer": "another answer"}]),
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 1);
    assert_eq!(asking_creates(&world), 1);
}

#[test]
fn busy_input_is_untouched_and_a_recovered_pending_question_is_never_delivered() {
    let world = World::new();
    let (_dir, _runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (wait, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([
            {"condition": wait, "question": "First question?", "answer": "first answer"},
            {"condition": host, "question": "Recovered while waiting?", "answer": "second answer"}
        ]),
    );
    until(&world, "first question", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    let terminal = asking_state(&world)["session"]["terminal"].as_str().unwrap().to_string();
    world.with_orca(|orca| {
        for row in orca["terminals"].as_array_mut().unwrap() {
            if row["handle"] == json!(terminal) {
                row["tui_busy"] = json!(true);
            }
        }
    });
    settle(&world, "settled");
    until(&world, "second question current but undelivered", || {
        let current = asking_state(&world)["current"].clone();
        (current["question"]["human"]["question"] == "Recovered while waiting?"
            && current["delivered"] == false)
            .then_some(())
    });
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 1);
    unpaused(&world, "pi");
    until(&world, "recovered pending question removed", || {
        asking_state(&world)["current"].is_null().then_some(())
    });
    world.with_orca(|orca| {
        for row in orca["terminals"].as_array_mut().unwrap() {
            if row["handle"] == json!(terminal) {
                row["tui_busy"] = json!(false);
            }
        }
    });
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 1);
}

#[test]
fn an_unverified_terminal_pauses_delivery_until_its_identity_is_restored() {
    let world = World::new();
    let (_dir, _runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (wait, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([
            {"condition": wait, "question": "First question?", "answer": "first answer"},
            {"condition": host, "question": "Second question?", "answer": "second answer"}
        ]),
    );
    until(&world, "first question", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    let handle = asking_state(&world)["session"]["terminal"].as_str().unwrap().to_string();
    world.with_orca(|orca| {
        let terminal = orca["terminals"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["handle"] == handle)
            .unwrap();
        terminal["ptyId"] = json!("different-terminal");
    });
    until(&world, "unverified association", || {
        asking_state(&world)["unavailable"].as_str().map(str::to_string)
    });
    settle(&world, "settled");
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 1);
    world.with_orca(|orca| {
        let terminal = orca["terminals"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["handle"] == handle)
            .unwrap();
        terminal["ptyId"] = json!(handle);
    });
    until(&world, "second question after identity is restored", || {
        let current = asking_state(&world)["current"].clone();
        (current["question"]["human"]["question"] == "Second question?"
            && current["delivered"] == true)
            .then_some(())
    });
    assert_eq!(asking_terminal(&world)["sent"].as_array().unwrap().len(), 2);
}

/// Two questions, the first delivered: the asking terminal's handle, and
/// the journal of its launch and delivery.
fn first_delivered(world: &World) -> (String, String) {
    let watch = until(world, "watch", || watch_running(world));
    let (wait, host) = conditions(&watch);
    report_humans(
        world,
        &watch,
        json!([
            {"condition": wait, "question": "First question?", "answer": "first answer"},
            {"condition": host, "question": "Second question?", "answer": "second answer"}
        ]),
    );
    until(world, "first question", || {
        (asking_state(world)["current"]["delivered"] == true).then_some(())
    });
    let state = asking_state(world);
    let session = state["session"]["id"].as_str().unwrap().to_string();
    (state["session"]["terminal"].as_str().unwrap().to_string(), session)
}

fn journal(world: &World) -> Value {
    serde_json::from_slice(
        &std::fs::read(world.cache().join("afkconvoy/asking/terminal.json")).unwrap(),
    )
    .unwrap()
}

/// The asking session's marked executor, as its wrapper is in a real pane:
/// the fake starts no harness. Ended when dropped.
struct Executor(std::process::Child);

impl Executor {
    fn of(session: &str) -> Self {
        let marker = format!("afkconvoy-watch-session-{session}");
        // One process, its command line carrying the marker: killing it
        // leaves no child behind.
        let child = std::process::Command::new("perl")
            .args(["-e", "sleep 120", &marker])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        Executor(child)
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The asking terminal's row comes back under a new handle and incarnation,
/// keeping `pty`, its tab and its leaf only as given.
fn reincarnate(world: &World, old: &str, pty: &str, tab: &str) {
    world.with_orca(|orca| {
        let terminal = orca["terminals"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["handle"] == old)
            .unwrap();
        terminal["ptyId"] = json!(pty);
        terminal["tabId"] = json!(tab);
        terminal["handle"] = json!("restarted-handle");
        terminal["incarnationId"] = json!("restarted-incarnation");
    });
}

fn restarted(world: &World) -> Value {
    world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["handle"] == "restarted-handle")
        .unwrap()
        .clone()
}

/// The session inside the asking terminal restarted in the same pty
/// (sunfmin/control#836): Orca lists the same ptyId, tab and leaf under a
/// new handle and incarnation. Its executor still runs, so the pane is taken
/// back: nothing is resent and the session goes on running in it.
#[test]
fn an_asking_terminal_back_in_the_same_pty_under_a_new_incarnation_is_taken_back() {
    let world = World::new();
    let (_dir, _runner) = two_conditions(&world);
    let (old, session) = first_delivered(&world);
    let _executor = Executor::of(&session);
    reincarnate(&world, &old, &old, &old);
    until(&world, "the restarted pane taken back", || {
        let state = asking_state(&world);
        (state["session"]["terminal"] == "restarted-handle" && state["unavailable"].is_null())
            .then_some(())
    });
    let journal = journal(&world);
    assert_eq!(journal["status"], "running");
    assert_eq!(journal["session"]["terminal"], "restarted-handle");
    assert_eq!(journal["terminal"]["incarnationId"], "restarted-incarnation");
    assert_eq!(asking_state(&world)["session"]["id"], session);
    assert_eq!(restarted(&world)["sent"].as_array().unwrap().len(), 1, "nothing resent");
    assert_eq!(asking_creates(&world), 1);
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["asking"]["status"], "asking", "{doctor}");
    settle(&world, "settled");
    until(&world, "second question in the restarted pane", || {
        let current = asking_state(&world)["current"].clone();
        (current["question"]["human"]["question"] == "Second question?"
            && current["delivered"] == true)
            .then_some(())
    });
    assert_eq!(restarted(&world)["sent"].as_array().unwrap().len(), 2);
    assert_eq!(asking_creates(&world), 1);
}

/// A terminal with the asking title in another pty, or in the same pty under
/// another tab, is not the asking session's, whatever it is named.
#[test]
fn a_same_name_terminal_in_another_pty_or_pane_is_still_refused() {
    for (pty, tab) in [(Some("another-pty"), None), (None, Some("another-tab"))] {
        let world = World::new();
        let (_dir, _runner) = two_conditions(&world);
        let (old, session) = first_delivered(&world);
        let _executor = Executor::of(&session);
        reincarnate(&world, &old, pty.unwrap_or(&old), tab.unwrap_or(&old));
        let why = until(&world, "refused association", || {
            asking_state(&world)["unavailable"].as_str().map(str::to_string)
        });
        assert!(why.contains("ownership is unverified"), "{why}");
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(asking_state(&world)["session"]["terminal"], json!(old));
        assert_eq!(journal(&world)["session"]["terminal"], json!(old));
        assert_eq!(restarted(&world)["sent"].as_array().unwrap().len(), 1);
        assert!(restarted(&world)["open"] == true, "a terminal not ours is left open");
        assert_eq!(asking_creates(&world), 1);
    }
}

/// The same pane under a new incarnation, but the session's executor is
/// gone: the pane is ours and empty of it. It is closed, and a fresh asking
/// session asks the current question again.
#[test]
fn a_restored_asking_pane_whose_executor_is_gone_is_closed_and_replaced() {
    let world = World::new();
    let (_dir, _runner) = two_conditions(&world);
    let (old, session) = first_delivered(&world);
    reincarnate(&world, &old, &old, &old);
    until(&world, "a fresh asking session", || {
        let state = asking_state(&world);
        (state["session"]["id"] != json!(session)
            && state["current"]["delivered"] == true
            && state["current"]["question"]["human"]["question"] == "First question?")
            .then_some(())
    });
    assert_eq!(restarted(&world)["open"], false, "the restored pane closed");
    assert_eq!(restarted(&world)["sent"].as_array().unwrap().len(), 1);
    assert_eq!(asking_creates(&world), 2);
}

#[test]
fn a_condition_recovered_after_delivery_is_told_as_withdrawn() {
    let world = World::new();
    let (_dir, _runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (_, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([{"condition": host, "question": "Approve this account?", "answer": "afkconvoy kit"}]),
    );
    until(&world, "delivered question", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    unpaused(&world, "pi");
    until(&world, "withdrawal delivered", || {
        let terminal = asking_terminal(&world);
        (terminal["sent"].as_array().unwrap().len() == 2).then_some(())
    });
    assert!(asking_brief(&world).contains("question withdrawn"));
    assert!(push_text(&asking_brief(&world)).starts_with("Withdrawn: Approve this account?"));
    until(&world, "withdrawn question cleared", || {
        asking_state(&world)["current"].is_null().then_some(())
    });
    assert_eq!(asking_creates(&world), 1);
}

#[test]
fn a_question_put_aside_stays_pending_and_does_not_hold_back_the_next() {
    let world = World::new();
    let (_dir, _runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (wait, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([
            {"condition": wait, "question": "Decide later?", "answer": "first answer"},
            {"condition": host, "question": "Decide now?", "answer": "second answer"}
        ]),
    );
    until(&world, "first question", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    settle(&world, "aside");
    until(&world, "next question after put aside", || {
        let current = asking_state(&world)["current"].clone();
        (current["question"]["human"]["question"] == "Decide now?" && current["delivered"] == true)
            .then_some(())
    });
    let state = asking_state(&world);
    assert_eq!(state["pending"].as_array().unwrap().len(), 1);
    assert_eq!(state["pending"][0]["aside"], true);
    assert_eq!(state["pending"][0]["question"]["human"]["question"], "Decide later?");
    assert!(asking_brief(&world).contains("Decide later?"));
    settle(&world, "settled");
    until(&world, "only the put-aside question left", || {
        (asking_state(&world)["current"].is_null()
            && asking_state(&world)["pending"].as_array().unwrap().len() == 1)
            .then_some(())
    });
    assert_eq!(asking_creates(&world), 1);
}

#[test]
fn a_settled_question_records_its_sessions_usage() {
    let world = World::new();
    let (dir, _runner) = two_conditions(&world);
    let watch = until(&world, "watch", || watch_running(&world));
    let (_, host) = conditions(&watch);
    report_humans(
        &world,
        &watch,
        json!([{"condition": host, "question": "Approve this account?", "answer": "afkconvoy kit"}]),
    );
    until(&world, "delivered question", || {
        (asking_state(&world)["current"]["delivered"] == true).then_some(())
    });
    let current = asking_state(&world)["current"].clone();
    let start = current["started"].as_u64().unwrap();
    let question = current["question"]["id"].as_str().unwrap();
    let session = asking_state(&world)["session"].clone();
    assert_eq!(session["harness"], "claude-code");
    let brief = world
        .cache()
        .join("afkconvoy/asking/sessions")
        .join(session["id"].as_str().unwrap())
        .join(format!("question-{question}.md"));
    let pointer = afkconvoy_core::prompt::brief_pointer(&brief.display().to_string());
    let harness = afkconvoy_core::harness::Harness::ClaudeCode;
    let recorded = &harness.session_files(&dir.display().to_string(), true, start)[0];
    let root = world.home().join(harness.session_place(&dir.display().to_string()).under_home);
    let file = root.join(&recorded.path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut text = recorded.body.replace("\"brief\"", &serde_json::to_string(&pointer).unwrap());
    text.push_str(&format!(
        "{}\n",
        json!({"timestamp": start, "type": "assistant", "cwd": dir,
            "isSidechain": false, "message": {"id": "answer", "role": "assistant",
            "model": "fixture", "usage": {"input_tokens": 9, "cache_read_input_tokens": 4,
            "cache_creation_input_tokens": 2, "output_tokens": 5}}})
    ));
    std::fs::write(&file, text).unwrap();
    settle(&world, "settled");
    until(&world, "question settled", || asking_state(&world)["current"].is_null().then_some(()));
    let usage: Value = serde_json::from_slice(
        &std::fs::read(
            world.cache().join("afkconvoy/asking/reports").join(question).join("usage.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(usage["id"], question);
    assert_eq!(usage["start"], start);
    assert_eq!(
        usage["tokens"]["fixture"],
        json!({"input": 9, "cache_read": 4, "cache_write": 2, "output": 5, "reasoning": null})
    );
    let accounts = world.afk(&["accounts", "--all", "--now", &usage["end"].to_string()]).ok();
    assert_eq!(accounts["asking_questions"]["count"], 1);
    assert_eq!(accounts["asking_questions"]["totals"]["input"], 9);
    assert_eq!(accounts["asking_questions"]["top"][0]["id"], question);
}

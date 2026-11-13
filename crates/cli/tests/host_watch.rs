//! The host runner watches its host: no model while a look finds nothing to
//! act on, and one watch in the operator's directory when one does — kept
//! across a restart of the host runner, continued on another harness where
//! its own gives out, and ended at its hour. The stand-in orca runs no
//! command: tests submit reports through the real CLI and supply terminal evidence.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use afkconvoy_core::host_watch::EXITED;
use serde_json::{Value, json};

use crate::common::{World, asked};

#[test]
fn two_hosts_find_one_wait_and_only_its_holder_reports_notifies_and_asks_it() {
    let first = World::new();
    first.issue(11, &["ready-for-human"]);
    first.comment(11, "<!--convoy:escalation claim=old-->\nApprove the account?");
    let dir = operator(&first);
    let output = first.root.join("notifications.jsonl");
    let command = append_notification(&output);
    first
        .afk(&[
            "run",
            "--host",
            "h1",
            "--watch-dir",
            dir.to_str().unwrap(),
            "--watch-every",
            "1",
            "--notify-command",
            &command,
        ])
        .ok();
    let round = until(&first, "holder's wait round", || {
        running(&first).filter(|w| {
            w["items"].as_array().is_some_and(|items| {
                items.iter().any(|i| i["id"].as_str().is_some_and(|id| id.contains("[\"human\",")))
            })
        })
    });

    let second = World::new();
    crate::common::git(
        &second.clone,
        &["remote", "set-url", "origin", first.bare.to_str().unwrap()],
    );
    let mut gh = first.gh();
    gh["calls"] = json!([]);
    second.write_gh(&gh);
    second.say_host_settings(json!({"host_name": "h2"}));
    let other_dir = operator(&second);
    let other_output = second.root.join("notifications.jsonl");
    second
        .afk(&[
            "run",
            "--host",
            "h2",
            "--watch-dir",
            other_dir.to_str().unwrap(),
            "--watch-every",
            "1",
            "--notify-command",
            &append_notification(&other_output),
        ])
        .ok();
    until(&second, "nonholder's completed look", || state(&second)["looked"].as_u64());
    let doctor = second.afk(&["doctor", "--host", "h2"]).json;
    assert_eq!(doctor["waiting_on_human"][0]["issue"], 11);
    assert_eq!(doctor["waiting_on_human"][0]["taken_by"], "h1");
    assert!(
        doctor["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "wait_taken"
                && f["severity"] == "info"
                && f["what"].as_str().unwrap().contains("h1")
        }),
        "{doctor}"
    );
    assert!(
        state(&second)["observed"]
            .as_object()
            .unwrap()
            .keys()
            .all(|id| !id.contains("[\"human\","))
    );
    assert!(state(&second)["rounds"].as_array().into_iter().flatten().all(|w| {
        w["items"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|i| !i["id"].as_str().unwrap().contains("[\"human\","))
    }));
    assert!(notify_state(&second)["entries"].as_object().is_none_or(|e| e.is_empty()));
    assert!(!other_output.exists());
    assert!(
        second.orca()["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["title"] != "afkconvoy asks")
    );

    // An unrelated local condition can start h2's watch, but its full look
    // and answering instructions must still omit h1's wait.
    paused(&second, "pi", "this host needs its own account");
    let local = until(&second, "nonholder's local host round", || {
        running(&second).filter(|w| {
            w["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["reason"].as_str().is_some_and(|reason| reason.contains("pi")))
            })
        })
    });
    assert_eq!(local["look"]["checkouts"][0]["doctor"]["waiting_on_human"], json!([]));
    report_humans(
        &second,
        &local,
        json!([{"question": "Provide h2's account?", "answer": "host answer"}]),
    );
    until(&second, "local host condition's notification", || {
        (notifications(&other_output).len() == 1).then_some(())
    });
    assert!(
        !notifications(&other_output)[0]["condition"].as_str().unwrap().contains("[\"human\",")
    );

    report_humans(
        &first,
        &round,
        json!([{"question": "Approve the account?", "answer": "answer"}]),
    );
    until(&first, "holder notification", || (notifications(&output).len() == 1).then_some(()));
    until(&first, "holder asking question", || {
        let state: Value = std::fs::read(first.cache().join("afkconvoy/asking/state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())?;
        (state["current"]["delivered"] == true).then_some(())
    });
    first.close_issue(11);
    second.close_issue(11);
    until(&first, "answered wait's claim removed", || {
        first.remote(&["for-each-ref", "refs/convoy/wait/"]).is_empty().then_some(())
    });
}

/// The operator's own terminal in the operator's directory: no watch ends it.
const OPERATORS: &str = "operators-own";

/// The operator's directory, known to orca as a repo's main checkout, with a
/// terminal of the operator's own open in it.
fn operator(world: &World) -> PathBuf {
    let dir = world.root.join("operator");
    std::fs::create_dir_all(&dir).unwrap();
    world.with_orca(|orca| {
        orca["worktrees"].as_array_mut().unwrap().push(
            json!({"path": dir, "repoId": "operator", "isMainWorktree": true, "isArchived": false}),
        );
        orca["terminals"].as_array_mut().unwrap().push(
            json!({"handle": OPERATORS, "worktreePath": dir, "command": "claude", "sent": [],
                "open": true}),
        );
    });
    dir
}

/// `run` with this host watching itself from `dir`, looking every second.
fn run(world: &World, dir: &Path) -> Value {
    let dir = dir.to_str().unwrap();
    let run = ["run", "--host", "h1", "--second-ms", "100", "--watch-dir", dir];
    world.afk(&[&run[..], &["--watch-every", "1"]].concat()).ok()
}

fn until<T>(world: &World, what: &str, mut got: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(found) = got() {
            return found;
        }
        let log = world.runner_log().join("\n");
        assert!(Instant::now() < until, "no {what} within 60s: {}\n{log}", state(world));
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Pauses `harness` on this host now: its probe is not due for an hour, so it
/// stays paused, held by the host kit or not. `doctor` finds it paused.
fn paused(world: &World, harness: &str, why: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    world.pause(harness, now.as_secs(), why);
}

/// The watch as the host keeps it.
fn state(world: &World) -> Value {
    let file = world.cache().join("afkconvoy/watch/state.json");
    std::fs::read(file).map_or(json!({}), |bytes| serde_json::from_slice(&bytes).unwrap())
}

fn running(world: &World) -> Option<Value> {
    Some(state(world)["running"].clone()).filter(|running| !running.is_null())
}

fn guarded_watch(harness: afkconvoy_core::harness::Harness) {
    let world = World::new();
    crate::asking_guard::prepare(&world, harness);
    world.issue(11, &["ready-for-human"]);
    world.comment(11, "<!--convoy:escalation claim=old-->\nAn operator decision is needed.");
    let dir = operator(&world);
    run(&world, &dir);
    until(&world, "guarded watch launch", || running(&world));
    let journal: Value = serde_json::from_slice(
        &std::fs::read(world.cache().join("afkconvoy/watch/terminal.json")).unwrap(),
    )
    .unwrap();
    crate::asking_guard::check_launch(
        &world,
        harness,
        &dir,
        journal["command"].as_str().unwrap(),
        true,
    );
}

macro_rules! guarded_watches {
    ($($name:ident: $harness:ident),* $(,)?) => { $(
        #[test]
        fn $name() { guarded_watch(afkconvoy_core::harness::Harness::$harness); }
    )* };
}

guarded_watches! {
    asking_guard_claude: ClaudeCode, asking_guard_pi: Pi, asking_guard_codex: Codex,
    asking_guard_dsh: Dsh, asking_guard_kimi: Kimi, asking_guard_grok: Grok,
}

fn ended(world: &World, outcome: &str) -> Option<Value> {
    Some(state(world)["last"].clone()).filter(|last| last["outcome"] == outcome)
}

/// The line each terminal started for a watch runs, as orca was asked to.
fn starts(world: &World) -> Vec<String> {
    let calls = world.orca_called_with().into_iter().map(|(call, _)| call);
    let watch = |call: &Vec<String>| call.windows(2).any(|w| w == ["--title", "afkconvoy watch"]);
    let calls = calls.filter(|call| asked(call) == "terminal create" && watch(call));
    let line = |call: Vec<String>| {
        let at = call.iter().position(|arg| arg == "--command").unwrap();
        call[at + 1].clone()
    };
    calls.map(line).collect()
}

/// The terminals open in `dir`, by handle.
fn open(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap().iter();
    let here = terminals.filter(|t| t["open"] == true && t["worktreePath"] == json!(dir));
    here.map(|t| t["handle"].as_str().unwrap().to_string()).collect()
}

/// What the brief a watch's session was pointed at says.
fn brief(world: &World, line: &str) -> String {
    let orca = world.orca();
    let terminal = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["line"] == line)
        .unwrap();
    let line = terminal["sent"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap();
    let at =
        line.find("Your task brief is the file ").unwrap() + "Your task brief is the file ".len();
    let file = &line[at..line[at..].find(" — ").unwrap() + at];
    std::fs::read_to_string(file).unwrap()
}

/// A successful session reports by command; a stopped or failed one leaves
/// terminal evidence and, if it exited, its shell-written exit status.
fn printed(world: &World, watch: &Value, lines: &[&str], status: Option<i32>) {
    if status == Some(0) {
        let file = world.root.join("watch-report.json");
        std::fs::write(
            &file,
            json!({"held_back": ["the look's finding"], "did": lines,
            "verified": ["checked the work goes on"], "human": [], "pending": []})
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
        return;
    }
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == watch["terminal"] {
                terminal["screen"] = json!(lines);
            }
        }
    });
    let mut text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if let Some(status) = status {
        text.push_str(&format!("{EXITED} {status}\n"));
    }
    std::fs::write(watch["transcript"].as_str().unwrap(), text).unwrap();
}

fn notify_state(world: &World) -> Value {
    std::fs::read(world.cache().join("afkconvoy/watch/notify.json"))
        .map_or(json!({}), |bytes| serde_json::from_slice(&bytes).unwrap())
}

fn notifications(file: &Path) -> Vec<Value> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn append_notification(file: &Path) -> String {
    afkconvoy_core::line::Line::new().fixed("cat >> ").word(file.to_str().unwrap()).own()
}

fn report_humans(world: &World, watch: &Value, mut human: Value) {
    for entry in human.as_array_mut().unwrap() {
        if entry.get("condition").is_none() {
            entry["condition"] = watch["items"][0]["id"].clone();
        }
    }
    let file = world.root.join("human-report.json");
    std::fs::write(
        &file,
        json!({"held_back": [], "did": [], "verified": [],
        "human": human, "pending": []})
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
    until(world, "the human report to settle", || {
        ended(world, "ended").filter(|last| last["id"] == watch["id"])
    });
}

/// Force the same observed fault into a new round after replacing the
/// runner. Its durable notification records are left untouched.
fn another_notification_round(world: &World, dir: &Path, previous: &Value) -> Value {
    world.kill_host_runner("KILL");
    let mut kept = state(world);
    kept["covered"] = json!([]);
    kept["watched"] = json!([]);
    std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
    run(world, dir);
    until(world, "another notification round", || {
        running(world)
            .filter(|w| w["id"] != previous["id"] && w["started"].as_u64().unwrap_or(0) > 0)
    })
}

#[test]
fn human_notifications_send_each_condition_once_despite_reworded_reports_and_restart() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "an account is needed");
    world.issue(11, &["ready-for-human"]);
    world.comment(11, "<!--convoy:escalation claim=old-->\nNeeds a quiet measurement environment.");
    let file = world.root.join("notifications.jsonl");
    let command = append_notification(&file);
    let ran = world
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
    let first = until(&world, "first notification round", || running(&world));
    let ids = first["conditions"].as_object().unwrap();
    let wait = ids.keys().find(|id| id.contains("\"human\"")).unwrap();
    let host = ids.keys().find(|id| !id.contains("\"human\"")).unwrap();
    let humans = json!([
        {"condition": wait, "question": "Provide the environment for #11?", "answer": "afkconvoy rule --issue 11 --text-file <file>"},
        {"condition": host, "question": "Approve the spend? $(touch should-never-exist)", "answer": "second answer"}
    ]);
    let invalid = world.root.join("invalid-human-report.json");
    let mut ambiguous = humans.clone();
    ambiguous[0].as_object_mut().unwrap().remove("condition");
    let invalid_report =
        |human| json!({"held_back": [], "did": [], "verified": [], "human": human, "pending": []});
    for invalid_humans in
        [ambiguous, json!([{"condition": "invented condition", "question": "?", "answer": "!"}])]
    {
        std::fs::write(&invalid, invalid_report(invalid_humans).to_string()).unwrap();
        assert!(
            world
                .afk(&[
                    "watched",
                    "--watch",
                    first["id"].as_str().unwrap(),
                    "--text-file",
                    invalid.to_str().unwrap()
                ])
                .error()
                .contains("condition")
        );
    }
    report_humans(&world, &first, humans.clone());
    let before = notify_state(&world)["look"].as_u64().unwrap();
    until(&world, "looks with no command named", || {
        notify_state(&world)["look"].as_u64().filter(|look| *look > before + 1)
    });
    assert!(!file.exists(), "no command named by default, so nothing runs");
    assert_eq!(notify_state(&world)["last"], Value::Null);
    let enabled = world.afk(&["run", "--notify-command", &command]).ok();
    assert_eq!(enabled["settings"]["notify_command"]["to"], command);
    until(&world, "two successful sendings", || {
        let entries = notify_state(&world)["entries"].clone();
        (entries.as_object().is_some_and(|e| e.len() == 2 && e.values().all(|v| v["sent"] == true)))
            .then_some(())
    });
    let sent = notifications(&file);
    assert_eq!(sent.len(), 2);
    for human in humans.as_array().unwrap() {
        let sent = sent.iter().find(|sent| sent["condition"] == human["condition"]).unwrap();
        assert_eq!(sent["question"], human["question"]);
        assert_eq!(sent["answer"], human["answer"]);
        assert_eq!(sent["watch"], first["id"]);
        assert_eq!(sent["host"], world.checkout_list()["settings"]["host_name"]);
        assert_eq!(sent["panel"], ran["host_runner"]["panel"]);
    }
    let issue = sent.iter().find(|sent| sent["condition"] == json!(wait)).unwrap();
    assert_eq!(issue["url"], "https://gh/c/2000");
    assert!(!world.clone.join("should-never-exist").exists());
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["notify"]["command"], command);
    assert_eq!(doctor["host"]["settings"]["in_force"]["notify_command"], command);
    assert_eq!(doctor["host"]["notify"]["last"]["outcome"], "sent");
    assert!(doctor["host"]["notify"]["last"]["ended"].as_u64().is_some());

    world.kill_host_runner("KILL");
    paused(&world, "kimi", "an unrelated new fault");
    run(&world, &dir);
    let second = until(&world, "another round for an unrelated condition", || {
        running(&world).filter(|w| w["id"] != first["id"] && w["started"].as_u64().unwrap_or(0) > 0)
    });
    assert!(
        !second["items"].as_array().unwrap().iter().any(|item| item["id"] == json!(wait)),
        "the already covered wait need not be the reason for this round"
    );
    let mut reworded = humans.clone();
    reworded[0]["question"] = json!("The same account, asked differently?");
    reworded[0]["answer"] =
        json!("cd /another/path && afkconvoy rule -C /repo --issue 11 --text-file <ruling>");
    reworded[1]["question"] = json!("A different way to ask about the spend?");
    reworded[1]["answer"] = json!("After supplying the account, run afkconvoy kit");
    report_humans(&world, &second, reworded);
    let before = notify_state(&world)["look"].as_u64().unwrap();
    until(&world, "later looks without repeated sending", || {
        notify_state(&world)["look"].as_u64().filter(|look| *look > before + 1)
    });
    assert_eq!(notifications(&file).len(), 2);

    let empty = another_notification_round(&world, &dir, &second);
    report_humans(&world, &empty, json!([]));
    assert_eq!(
        notify_state(&world)["entries"].as_object().unwrap().len(),
        2,
        "omitting a decision from one report does not end its condition's episode"
    );
    std::fs::write(world.paused_path(), "[]").unwrap();
    world.close_issue(11);
    until(&world, "observed recovery ends both episodes", || {
        (notify_state(&world)["entries"] == json!({})).then_some(())
    });
    world.kill_host_runner("KILL");
    paused(&world, "pi", "an account is needed");
    world.with_gh(|gh| {
        gh["issues"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|issue| issue["number"] == 11)
            .unwrap()["state"] = json!("open");
    });
    run(&world, &dir);
    let returned = until(&world, "a new episode of each condition", || {
        running(&world).filter(|w| w["id"] != empty["id"] && w["started"].as_u64().unwrap_or(0) > 0)
    });
    report_humans(&world, &returned, humans);
    until(&world, "returned decisions sent again", || {
        (notifications(&file).len() == 4).then_some(())
    });

    world.afk(&["run", "--notify-command", "default"]).ok();
    let disabled = another_notification_round(&world, &dir, &returned);
    report_humans(
        &world,
        &disabled,
        json!([{"question": "New decision?", "answer": "unsent answer"}]),
    );
    let before = notify_state(&world)["look"].as_u64().unwrap();
    until(&world, "looks with sending disabled", || {
        notify_state(&world)["look"].as_u64().filter(|look| *look > before + 1)
    });
    assert_eq!(notifications(&file).len(), 4);
    assert_eq!(world.afk(&["doctor"]).json["host"]["notify"]["command"], Value::Null);
}

fn one_notification(world: &World) -> Value {
    notify_state(world)["entries"]
        .as_object()
        .and_then(|entries| entries.values().next())
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn human_notification_failures_retry_at_later_looks_and_doctor_finds_three_then_recovers() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "an account is needed");
    let file = world.root.join("attempts.jsonl");
    let command = format!("{}; exit 17", append_notification(&file));
    world.say_host_settings(json!({"notify_command": command}));
    run(&world, &dir);
    let watch = until(&world, "notification round", || running(&world));
    report_humans(&world, &watch, json!([{"question": "Supply an account?", "answer": "answer"}]));
    until(&world, "three failed sendings", || {
        one_notification(&world)["failures"].as_u64().filter(|failures| *failures >= 3)
    });
    let before = state(&world)["looked"].as_u64().unwrap();
    let doctor = world.afk(&["doctor"]).json;
    let finding = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "notify_failed")
        .unwrap();
    assert!(finding["fix"].as_str().unwrap().contains("--notify-command"));
    assert!(doctor["host"]["notify"]["last"]["outcome"].as_str().unwrap().contains("failed"));
    assert!(notifications(&file).len() >= 3);
    assert!(running(&world).is_none(), "failed sending leaves the watch standing by");

    let recovered = world.root.join("recovered.jsonl");
    let command = append_notification(&recovered);
    world.afk(&["run", "--notify-command", &command]).ok();
    until(&world, "successful retry after changing the command", || {
        (one_notification(&world)["sent"] == true).then_some(())
    });
    assert_eq!(notifications(&recovered).len(), 1);
    assert_eq!(one_notification(&world)["failures"], 0);
    assert!(state(&world)["looked"].as_u64().unwrap() > before);
    assert!(
        !world.afk(&["doctor"]).json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "notify_failed")
    );
}

#[test]
fn a_hung_human_notification_times_out_and_retries_without_holding_back_looks_or_rounds() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "an account is needed");
    let file = world.root.join("hung.jsonl");
    let command = format!("{}; sleep 120", append_notification(&file));
    world.say_host_settings(json!({"notify_command": command}));
    run(&world, &dir);
    let first = until(&world, "notification round", || running(&world));
    let human = json!([{"question": "Supply an account?", "answer": "answer"}]);
    report_humans(&world, &first, human.clone());
    until(&world, "hung command started", || (notifications(&file).len() == 1).then_some(()));
    let before = state(&world)["looked"].as_u64().unwrap();
    paused(&world, "kimi", "another fault while sending hangs");
    let next = until(&world, "another round during the hung command", || {
        running(&world).filter(|w| w["id"] != first["id"] && w["started"].as_u64().unwrap_or(0) > 0)
    });
    report_humans(&world, &next, human);
    assert_eq!(notifications(&file).len(), 1, "another round completed within the sending bound");
    assert!(state(&world)["looked"].as_u64().unwrap() > before);
    until(&world, "timeout recorded and retried", || {
        (one_notification(&world)["failures"].as_u64().unwrap_or(0) >= 1
            && notifications(&file).len() >= 2)
            .then_some(())
    });
    assert!(!one_notification(&world)["sent"].as_bool().unwrap());
    world.afk(&["stop", "--all"]).ok();
}

#[test]
fn notify_command_environment_and_flag_follow_host_setting_precedence() {
    let world = World::new();
    let command = "exit 73";
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("AFKCONVOY_NOTIFY_COMMAND", command)
        .args(["run", "--tracker", "default", "--concurrency", "default"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(world.checkout_list()["settings"]["notify_command"], command);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("AFKCONVOY_NOTIFY_COMMAND", command)
        .args(["run", "--notify-command", "default"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(world.checkout_list()["settings"]["notify_command"], Value::Null);
    world.afk(&["stop", "--all"]).ok();
}

#[test]
fn a_host_with_no_operator_directory_runs_the_stock_watch_and_records_what_stays_human() {
    let world = World::new();
    let dir = world.cache().join("afkconvoy/stock-watch");
    // doctor resolves the directory without making or registering it.
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["watch"]["directory"], json!(dir));
    assert_eq!(doctor["host"]["watch"]["stock"], true);
    assert!(!dir.exists());
    world.issue(7, &["ready-for-human"]);
    world.comment(7, "<!--convoy:escalation claim=old-->\nChoose the schema.");
    paused(&world, "pi", "it refused every prompt");
    world.afk(&["run", "--host", "h1", "--second-ms", "100", "--watch-every", "1"]).ok();
    let watch = until(&world, "the stock watch", || running(&world));
    assert_eq!(watch["directory"], json!(dir));
    assert_eq!(watch["stock"], true);
    let rules = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(), rules);
    for required in [
        "Find what holds",
        "fix it with the watch's authority",
        "check that the work goes on",
        "say what you did",
        "spending money, or needing an account, a credential or a person's hands",
        "what cannot be undone",
        "taking a secret or private data off the host",
        "changing what a product is for, or who it is for",
        "never act on them",
        "list each in your report",
    ] {
        assert!(rules.contains(required), "stock instruction lacks {required}: {rules}");
    }
    let told = brief(&world, &starts(&world)[0]);
    assert!(told.contains("Answer an escalation yourself"));
    assert!(told.contains("afkconvoy rule --issue 7 --text-file <file>"));
    assert!(
        told.contains("gh issue edit 7 --add-label ready-for-agent --remove-label ready-for-human")
    );
    assert!(told.contains("Leave only these four kinds with a human"));
    assert_eq!(open(&world, &dir), [watch["terminal"].as_str().unwrap()]);
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["running"]["stock"], true);
    let settings = &doctor["checkouts"][0]["doctor"]["host"]["settings"];
    assert_eq!(settings["said"]["watch_dir"], Value::Null);
    assert_eq!(settings["in_force"]["watch_dir"], json!(dir));

    let left = [
        "left with a human: spending money to create an account and supply a credential",
        "left with a human: an action that cannot be undone",
        "left with a human: taking private data off the host",
        "left with a human: changing who the product is for",
    ];
    printed(&world, &watch, &left, Some(0));
    until(&world, "the stock watch's record", || ended(&world, "ended"));
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["last"]["did"], json!(left));
    assert_eq!(doctor["host"]["watch"]["last"]["stock"], true);
    assert_eq!(starts(&world).len(), 1, "one stock watch per episode");
    let adds = world.orca_called_with().into_iter().filter(|(call, _)| asked(call) == "repo add");
    assert_eq!(adds.count(), 1, "the stock directory is registered by the fleet");
    let help = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["run", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("stock watch"), "{help}");
    assert!(help.contains("stock-watch"), "{help}");
}

#[test]
fn the_operator_directory_wins_and_default_returns_to_the_stock_watch() {
    let world = World::new();
    let dir = operator(&world);
    let rules = "# This operator\nKeep the operator's rules as written.\n";
    std::fs::write(dir.join("CLAUDE.md"), rules).unwrap();
    paused(&world, "pi", "it refused every prompt");
    run(&world, &dir);
    let watch = until(&world, "the operator's watch", || running(&world));
    assert_eq!(watch["directory"], json!(dir));
    assert_eq!(watch["stock"], false);
    assert_eq!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), rules);
    assert!(!dir.join("AGENTS.md").exists());
    assert!(!world.cache().join("afkconvoy/stock-watch").exists());
    assert!(!brief(&world, &starts(&world)[0]).contains("# The stock watch"));
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["directory"], json!(dir));
    assert_eq!(doctor["host"]["watch"]["stock"], false);

    // The running watch keeps its own directory and rules. The next watch
    // uses the new setting, and the stock directory needs no registration.
    world.afk(&["run", "--host", "h1", "--watch-dir", "default"]).ok();
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["stock"], true);
    assert_eq!(doctor["host"]["watch"]["running"]["stock"], false);
    printed(&world, &watch, &["checked pi"], Some(0));
    paused(&world, "dsh", "another episode");
    let stock = until(&world, "the next stock watch", || {
        running(&world).filter(|watch| watch["stock"] == true)
    });
    assert_eq!(stock["directory"], json!(world.cache().join("afkconvoy/stock-watch")));
    assert_eq!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), rules);
    assert!(open(&world, &dir).contains(&OPERATORS.to_string()));
}

#[test]
fn a_look_reads_a_terminal_preview_ending_in_a_lone_surrogate() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "it refused every prompt");
    run(&world, &dir);
    let watch = until(&world, "the host watch", || running(&world));
    let first = state(&world)["until"].as_u64().unwrap();
    let answer = json!({"ok": true, "result": {"terminals": [{
        "handle": OPERATORS, "worktreePath": dir, "connected": true, "writable": true,
        "preview": "⏺SURROGATE"
    }]}})
    .to_string()
    .replace("SURROGATE", r"\udf13");
    world.with_orca(|orca| orca["raw_answers"] = json!({"terminal list": answer}));
    printed(&world, &watch, &["checked the host"], Some(0));
    let no_failures = || {
        for file in std::fs::read_dir(world.cache().join("afkconvoy/runs")).unwrap() {
            let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
            assert!(!text.contains("\"watch_failed\""), "{text}");
        }
    };
    until(&world, "a successful look", || {
        no_failures();
        state(&world)["until"].as_u64().filter(|until| *until > first)
    });
    assert!(world.orca_calls().iter().any(|call| asked(call) == "terminal list"));
    assert_eq!(state(&world)["last"]["outcome"], "ended");
    no_failures();
}

#[test]
fn a_look_with_nothing_to_act_on_starts_no_model_and_moves_its_until_on() {
    let world = World::new();
    world.afk(&["run", "--host", "h1", "--second-ms", "100", "--watch-every", "1"]).ok();
    let first = until(&world, "first look", || state(&world)["until"].as_u64());
    until(&world, "later look", || state(&world)["until"].as_u64().filter(|until| *until > first));
    assert!(starts(&world).is_empty(), "{:?}", starts(&world));
    assert!(state(&world)["running"].is_null() && state(&world)["last"].is_null());
    assert!(!world.cache().join("afkconvoy/stock-watch").exists());
}

#[test]
fn unplaced_models_start_a_daily_watch_that_places_research_and_keeps_its_bound_across_restarts() {
    let world = World::new();
    world.kit_models("claude-code", &["fast", "deep"]);
    world.with_kit(|kit| {
        for model in kit["harnesses"][0]["models"].as_array_mut().unwrap() {
            model["good_at"] = json!("research pending");
        }
    });
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "the watch of unplaced models", || running(&world));
    assert_eq!(watch["items"].as_array().unwrap().len(), 1, "{watch}");
    assert!(watch["items"][0]["id"].as_str().unwrap().starts_with("kit-research:"));
    assert_eq!(watch["model"], Value::Null, "the watch starts on the default too");
    let told = brief(&world, &starts(&world)[0]);
    for instruction in [
        "after anything holding work up",
        "Use your web tools",
        "afkconvoy kit --research <file>",
        "correct the file",
        "what remains unplaced",
    ] {
        assert!(told.contains(instruction), "{told}");
    }
    let file = world.root.join("research.json");
    std::fs::write(&file, json!({"models":[{"harness":"claude-code", "model":"fast",
        "good_at":"small fixes; limited deep reasoning", "sources":[{"url":"https://vendor.example/card", "date":"2026-10-24"}]}]}).to_string()).unwrap();
    let applied = world.afk(&["kit", "--research", file.to_str().unwrap()]).ok();
    assert_eq!(applied["placed"], json!([{"harness":"claude-code", "model":"fast"}]));
    printed(&world, &watch, &["placed fast; deep remains unplaced"], Some(0));
    until(&world, "research watch ended", || ended(&world, "ended"));
    let first_until = state(&world)["until"].as_u64().unwrap();
    until(&world, "a later look with research left", || {
        state(&world)["until"].as_u64().filter(|at| *at > first_until)
    });
    assert_eq!(starts(&world).len(), 1, "pending research alone waits a day");

    world.kill_host_runner("KILL");
    run(&world, &dir);
    let before = state(&world)["until"].as_u64().unwrap();
    until(&world, "a look after restart", || {
        state(&world)["until"].as_u64().filter(|at| *at > before)
    });
    assert_eq!(starts(&world).len(), 1, "restart keeps the research bound");

    // A new urgent item takes pending research even inside its daily bound.
    paused(&world, "pi", "a new fault");
    let urgent =
        until(&world, "a watch of the new fault and remaining research", || running(&world));
    let items = urgent["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{urgent}");
    assert!(items[0]["id"].as_str().unwrap().starts_with("finding:"));
    assert!(items[1]["id"].as_str().unwrap().starts_with("kit-research:"));
    printed(&world, &urgent, &["checked pi; deep has no evidence yet"], Some(0));
    until(&world, "the urgent watch ended", || {
        ended(&world, "ended").filter(|last| last["id"] == urgent["id"])
    });

    world.kill_host_runner("KILL");
    let mut kept = state(&world);
    kept["research_started"] = json!(
        kept["research_started"].as_u64().unwrap() - afkconvoy_core::host_watch::RESEARCH_EVERY
    );
    std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
    run(&world, &dir);
    let daily = until(&world, "research due again after a day", || running(&world));
    assert_eq!(daily["items"].as_array().unwrap().len(), 1, "{daily}");
    assert!(daily["items"][0]["id"].as_str().unwrap().starts_with("kit-research:"));
    assert_eq!(daily["model"], Value::Null, "the healthy resident keeps its model across rounds");

    std::fs::write(
        &file,
        json!({"models":[{"harness":"claude-code", "model":"deep",
        "settled":"approval reviewer, unsuited to workers",
        "sources":[{"url":"https://vendor.example/reviewer", "date":"2026-10-24"}]}]})
        .to_string(),
    )
    .unwrap();
    let settled = world.afk(&["kit", "--research", file.to_str().unwrap()]).ok();
    assert_eq!(settled["placed"], json!([]));
    assert_eq!(settled["settled"], json!([{"harness":"claude-code", "model":"deep"}]));
    printed(&world, &daily, &["settled deep with dated evidence"], Some(0));
    until(&world, "settling watch ended", || {
        ended(&world, "ended").filter(|last| last["id"] == daily["id"])
    });
    world.kill_host_runner("KILL");
    let mut kept = state(&world);
    kept["research_started"] = json!(
        kept["research_started"].as_u64().unwrap() - afkconvoy_core::host_watch::RESEARCH_EVERY
    );
    let before = kept["until"].as_u64().unwrap();
    std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
    run(&world, &dir);
    until(&world, "look after settlement, restart and daily bound", || {
        state(&world)["until"].as_u64().filter(|at| *at > before)
    });
    assert_eq!(
        starts(&world).len(),
        1,
        "all rounds reused one resident; settled models start no further round"
    );
    assert!(state(&world)["running"].is_null());
    let doctor = world.afk(&["doctor", "--all"]).ok();
    assert!(
        !doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["what"].as_str().unwrap().contains("unplaced models")),
        "{doctor}"
    );
}

#[test]
fn a_watch_answers_escalations_and_leaves_only_the_four_human_kinds() {
    let world = World::new();
    let dir = operator(&world);
    world.set_gate_config("gate_command: ./gate.sh\nescalate_to: human");
    let other = world.second_repo();
    world
        .afk_in(
            &other,
            &["run", "--host", "h1", "--watch-dir", dir.to_str().unwrap(), "--watch-every", "1"],
        )
        .ok();
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-human"]);
    world.comment(
        8,
        "<!--convoy:escalation claim=old-->\nShould we take the customer's data off this host?",
    );
    world.with_orca(|orca| orca["trust_prompt"] = json!(true));
    let fault = world.afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1"]);
    assert!(fault.error().contains("worktree trust gap"));
    // Register both checkouts before waiting for their look: every host
    // watches now, with its settings in force before the first tick.
    world.afk(&["run", "--host", "h1"]).ok();
    // The watch takes its brief once the stand-in host fault is corrected.
    world.with_orca(|orca| orca["trust_prompt"] = json!(false));
    run(&world, &dir);
    let watch = until(&world, "the watch of the fleet fault", || running(&world));
    let told = brief(&world, &starts(&world)[0]);
    for required in [
        "every repo on this host's checkout list",
        "trust a worktree for its harness",
        "waiting_on_human.answer",
        "actually dispatches or continues",
        "Answer an escalation yourself",
        "afkconvoy rule --issue 8 --text-file <file>",
        "afkconvoy send --issue 8 --stage <stage> --text-file <file>",
        "gh issue close 8 --comment <why>",
        "dependencies/blocked_by",
        "Leave only these four kinds with a human",
        "spending money, or needing an account, a credential or a person's hands",
        "what cannot be undone",
        "taking a secret or private data off the host",
        "changing what a product is for, or who it is for",
        "afkconvoy-watch-<digits>",
        "merge commit",
        "afkconvoy land --watch",
        "the next idle tick starts a due cut",
        "On the new version repeat the check",
        "never report the fix verified before checking it",
    ] {
        assert!(told.contains(required), "brief lacks {required}: {told}");
    }
    assert!(told.contains(other.to_str().unwrap()), "the look includes the other repo");
    assert!(told.contains("customer's data"), "the human's question is given whole: {told}");

    let doctor = world.afk(&["doctor"]).json;
    let waiting = doctor["waiting_on_human"].as_array().unwrap();
    let answer =
        waiting.iter().find(|item| item["issue"] == 7).unwrap()["answer"].as_str().unwrap();
    // The operator fixes the cause before running precisely the current
    // answer line. Only gh and orca are stood in for; dispatch is real.
    world.with_orca(|orca| orca["trust_prompt"] = json!(false));
    let handed = world.command("sh", &world.clone).args(["-c", answer]).output().unwrap();
    assert!(handed.status.success(), "{}", String::from_utf8_lossy(&handed.stderr));
    world.afk(&["wake"]).ok();
    until(&world, "the recovered issue to dispatch", || {
        let orca = world.orca();
        orca["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|terminal| {
                terminal["open"] == true
                    && terminal["sent"].as_array().is_some_and(|sent| {
                        sent.iter().any(|line| {
                            line["text"].as_str().is_some_and(|line| line.contains("task brief"))
                        })
                    })
            })
            .cloned()
    });
    let human = world.gh()["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["number"] == 8)
        .unwrap()
        .clone();
    assert_eq!(human["labels"], json!(["ready-for-human"]));
    printed(
        &world,
        &watch,
        &[
            "fixed trust and checked #7 took its brief",
            "left #8 with the human: private data leaving the host",
        ],
        Some(0),
    );
    let last = until(&world, "the watch's record", || ended(&world, "ended"));
    assert!(last["did"][1].as_str().unwrap().contains("left #8 with the human"));
}

/// sunfmin/control#801: a failed cut holds every later version back while
/// turns keep landing, so the fleet never stops for it. With the fleet
/// otherwise moving — an issue taken by its worker — the failure's issue,
/// the watch's to answer and no human's, starts a watch whose brief names
/// it: its issue, its step, why, and how the cut goes on.
#[test]
fn a_failed_cut_starts_a_watch_while_the_fleet_moves_and_its_brief_names_the_failure() {
    use afkconvoy_core::cut::{self, FAILED_LABEL, Failure, Record, Step};
    let world = World::new();
    let dir = operator(&world);
    let cuts = "```yaml cut\nset: |\n  true\npublish: |\n  true\n```\n";
    std::fs::write(
        world.clone.join("docs/agents/afkconvoy.md"),
        format!("{}\n{cuts}", crate::common::CONFIG),
    )
    .unwrap();
    crate::common::git(&world.clone, &["commit", "-q", "-am", "Turn cutting on (#3)"]);
    crate::common::git(&world.clone, &["push", "-q", "origin", crate::common::BASE]);
    let record: Record = serde_json::from_value(json!({"version": "0.1.1", "started": 1000}))
        .expect("a cut's record");
    let why = "the gate ran red: `steer::a` timed out";
    let line = "cd '/w/afkconvoy-cut' && afkconvoy cut --from gate";
    let body = cut::failure_report(
        &record,
        &Failure::at(Step::Gate, why),
        "h1",
        Some("/w/afkconvoy-cut"),
        line,
    );
    world.with_gh(|gh| {
        gh["issues"].as_array_mut().unwrap().push(json!({"number": 9,
            "title": cut::failure_title("0.1.1"), "labels": [FAILED_LABEL], "state": "open",
            "body": body}));
    });
    world.issue(7, &["ready-for-agent"]);

    run(&world, &dir);
    let watch = until(&world, "the watch of the failed cut", || running(&world));
    let items: Vec<&str> =
        watch["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(items.iter().any(|id| id.contains(r#"["cut-failed",9]"#)), "{items:?}");
    assert!(!items.iter().any(|id| id.contains(r#""human""#)), "no human waits: {items:?}");
    let told = brief(&world, &starts(&world)[0]);
    for required in [
        "the fleet cuts no version: its last cut failed, of v0.1.1 at its step `gate`",
        &format!("#9: the step `gate` failed: {why}"),
        "gh issue close 9",
        "## Answer a failed cut",
        &format!("Its issue, under the label `{FAILED_LABEL}`, is yours to answer"),
    ] {
        assert!(told.contains(required), "brief lacks {required}: {told}");
    }
    // The fleet moves all the while: it claimed #7 and started its worker.
    until(&world, "#7 claimed", || world.claims().into_iter().find(|(issue, _)| *issue == 7));
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["waiting_on_human"], json!([]), "{doctor}");
}

#[test]
fn a_reported_human_wait_sends_no_more_prompts_across_looks_and_runner_restart() {
    let world = World::new();
    let dir = operator(&world);
    world.issue(11, &["ready-for-human"]);
    world.comment(11, "<!--convoy:escalation claim=old-->\nNeeds a quiet measurement environment.");
    world.with_gh(|gh| gh["comments"]["11"][0]["at"] = json!(1_793_036_772u64));
    // Real doctor output at the two observed ages retains the warning,
    // reason, source and exact answer, but has one structured identity.
    let earlier = world.afk(&["doctor", "--now", "1793070792"]).json;
    let later = world.afk(&["doctor", "--now", "1793071152"]).json;
    let warning = |answer: &Value| {
        answer["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["what"].as_str().unwrap().contains("has waited for a human"))
            .unwrap_or_else(|| panic!("no long wait in {answer}"))
            .clone()
    };
    assert!(warning(&earlier)["what"].as_str().unwrap().contains("9h 27m"));
    assert!(warning(&later)["what"].as_str().unwrap().contains("9h 33m"));
    assert_eq!(warning(&earlier)["watch_id"], warning(&later)["watch_id"]);
    assert_eq!(warning(&later)["fix"], later["waiting_on_human"][0]["answer"]);
    assert_eq!(later["waiting_on_human"][0]["url"], "https://gh/c/2000");
    assert!(later["waiting_on_human"][0]["why"].as_str().unwrap().contains("quiet measurement"));

    run(&world, &dir);
    let first = until(&world, "human wait round", || {
        running(&world).filter(|w| w["started"].as_u64().unwrap_or(0) > 0)
    });
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let report = world.root.join("human-report.json");
    std::fs::write(&report, json!({"held_back": ["quiet measurement environment"], "did": [],
        "verified": ["human decision still pending"], "human": [{"question": "Provide the environment?",
        "answer": later["waiting_on_human"][0]["answer"]}], "pending": []}).to_string()).unwrap();
    world
        .afk(&[
            "watched",
            "--watch",
            first["id"].as_str().unwrap(),
            "--text-file",
            report.to_str().unwrap(),
        ])
        .ok();
    until(&world, "human report", || ended(&world, "ended"));
    let at = state(&world)["until"].as_u64().unwrap();
    until(&world, "repeated looks of the reported question", || {
        state(&world)["until"].as_u64().filter(|until| *until > at + 2)
    });
    assert!(running(&world).is_none());
    assert_eq!(sent(&world, &first).len(), 1, "no repeated model prompt");
    assert_eq!(state(&world)["pending"], json!([]));

    world.kill_host_runner("KILL");
    run(&world, &dir);
    let at = state(&world)["until"].as_u64().unwrap();
    until(&world, "repeated looks after runner replacement", || {
        state(&world)["until"].as_u64().filter(|until| *until > at + 2)
    });
    assert!(running(&world).is_none());
    assert_eq!(starts(&world).len(), 1);
    assert_eq!(sent(&world, &first).len(), 1);
    assert_eq!(world.labels(11), ["ready-for-human"]);

    // A new source needs a fresh answer even without an intervening look
    // that found the issue recovered.
    world.comment(11, "<!--convoy:escalation claim=new-->\nNeeds different measurement equipment.");
    world.with_gh(|gh| {
        gh["comments"]["11"].as_array_mut().unwrap().last_mut().unwrap()["at"] =
            json!(1_793_036_773u64)
    });
    let next = until(&world, "changed human question", || {
        running(&world).filter(|w| w["started"].as_u64().unwrap_or(0) > 0)
    });
    assert_ne!(next["id"], first["id"]);
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_ne!(next["episodes"], first["episodes"]);
    assert_eq!(sent(&world, &next).len(), 2);
    printed(&world, &next, &["left changed question for a human"], Some(0));
}

#[test]
fn one_watch_covers_an_episode_is_kept_across_a_restart_and_is_recorded() {
    let world = World::new();
    let dir = operator(&world);
    // Something an operator would act on: a harness the host kit does not
    // hold, paused all the same.
    paused(&world, "pi", "it refused every prompt");
    let first = run(&world, &dir);
    let watch = until(&world, "a watch", || running(&world));
    assert_eq!(watch["harness"], "claude-code");
    let started = starts(&world);
    assert_eq!(started.len(), 1, "{started:?}");
    let line = &started[0];
    assert!(!line.contains(" --print "), "interactive: {line}");
    assert!(line.contains("--dangerously-skip-permissions"), "trusted: {line}");
    assert!(!line.contains("tee -a"), "the TUI owns its terminal: {line}");
    let told = brief(&world, &started[0]);
    assert!(told.contains("the harness `pi` is paused on this host"), "{told}");
    assert!(told.contains("it refused every prompt"), "the look, whole: {told}");
    assert_eq!(
        open(&world, &dir),
        [OPERATORS.to_string(), watch["terminal"].as_str().unwrap().into()]
    );

    // What a look would find while it runs waits for it to end.
    paused(&world, "dsh", "it refused every prompt");
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(starts(&world).len(), 1, "no second watch beside the first");

    world.kill_host_runner("KILL");
    let again = run(&world, &dir);
    assert_ne!(again["host_runner"]["pid"], first["host_runner"]["pid"]);
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(starts(&world).len(), 1, "the running watch is kept, and none started beside it");
    assert_eq!(running(&world).unwrap()["id"], watch["id"]);

    printed(&world, &watch, &["looked at pi", "left dsh for the next look"], Some(0));
    let last = until(&world, "the watch to end", || ended(&world, "ended"));
    assert_eq!(last["id"], watch["id"]);
    assert_eq!(last["did"], json!(["looked at pi", "left dsh for the next look"]));
    assert!(open(&world, &dir).contains(&OPERATORS.to_string()), "the operator's own is kept");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["last"]["id"], watch["id"], "{}", doctor["host"]["watch"]);
    let log = std::fs::read_to_string(first["tick_log"].as_str().unwrap()).unwrap();
    for said in ["watch_started", "looked at pi", "watch_ended"] {
        assert!(log.contains(said), "{said}: {log}");
    }

    // pi's episode goes on, and starts nothing more; dsh's is new, and found
    // by the first look after the watch ended.
    let next = until(&world, "the next watch", || {
        running(&world).filter(|next| next["id"] != watch["id"])
    });
    let items: Vec<&str> =
        next["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].contains("\"dsh\""), "{items:?}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["running"]["id"], next["id"]);
}

#[test]
fn a_watch_closed_without_reporting_does_not_consume_the_unresolved_episode() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "the same unresolved fault");
    run(&world, &dir);
    let watch = until(&world, "the first watch", || running(&world));
    let first_look = state(&world)["until"].as_u64().unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == watch["terminal"] {
                terminal["open"] = json!(false);
            }
        }
    });
    until(&world, "the unreported watch to end", || ended(&world, "closed"));
    // Observe a completed look after that failure, rather than guessing how
    // long the host needs to notice it. The fault has not changed or cleared.
    let deadline = Instant::now() + Duration::from_secs(10);
    let after = loop {
        let after = state(&world);
        if after["until"].as_u64().unwrap_or(0) > first_look && after["running"].is_object() {
            break after;
        }
        assert!(Instant::now() < deadline, "no look after watch closure: {after}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        after["running"].is_object(),
        "an unreported watch must not suppress automatic recovery of the same fault: {after}"
    );
    assert_ne!(after["running"]["id"], watch["id"]);
    let ids = |watch: &Value| {
        watch["items"].as_array().unwrap().iter().map(|item| item["id"].clone()).collect::<Vec<_>>()
    };
    assert_eq!(ids(&after["running"]), ids(&watch));
    assert_eq!(starts(&world).len(), 2);
    assert!(open(&world, &dir).contains(&OPERATORS.to_string()));

    let next = &after["running"];
    printed(&world, next, &["checked the same fault and recorded the decision"], Some(0));
    until(&world, "the replacement's report", || {
        ended(&world, "ended").filter(|last| last["id"] == next["id"])
    });
    until(&world, "a look after the replacement reported", || {
        let state = state(&world);
        (state["until"].as_u64() > after["until"].as_u64()).then_some(state)
    });
    assert_eq!(starts(&world).len(), 2, "the later report covers the earlier failure");
    assert!(running(&world).is_none());
}

#[test]
fn a_restart_recovers_an_unreported_episode_kept_by_an_older_runner() {
    for research in [false, true] {
        let world = World::new();
        let dir = operator(&world);
        if research {
            world.kit_models("claude-code", &["fast"]);
            world.with_kit(|kit| {
                kit["harnesses"][0]["models"][0]["good_at"] = json!("research pending")
            });
        } else {
            paused(&world, "pi", "the same unresolved fault");
        }
        run(&world, &dir);
        let watch = until(&world, "the first watch", || running(&world));
        world.kill_host_runner("KILL");
        // An older runner has already ended this watch without a report, but
        // left its items covered. A now-available harness must get that work.
        let mut failed = watch.clone();
        failed["ended"] = watch["started"].clone();
        failed["outcome"] = json!("no_harness");
        let mut kept = state(&world);
        kept["running"] = Value::Null;
        kept["last"] = failed.clone();
        kept["history"] = json!([failed]);
        kept["unavailable"] = json!("no harness was available");
        std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                if terminal["handle"] == watch["terminal"] {
                    terminal["open"] = json!(false);
                }
            }
        });
        run(&world, &dir);
        let next = until(&world, "automatic recovery after restart", || running(&world));
        assert_ne!(next["id"], watch["id"]);
        assert_eq!(starts(&world).len(), 2);
        assert!(state(&world)["unavailable"].is_null());
        assert!(brief(&world, starts(&world).last().unwrap()).contains("It did not report."));
        assert!(open(&world, &dir).contains(&OPERATORS.to_string()));
    }
}

#[test]
fn a_watch_whose_harness_runs_out_goes_on_on_the_next_with_the_same_look() {
    let world = World::new();
    let dir = operator(&world);
    world.hold_every_harness();
    paused(&world, "kimi", "it refused every prompt");
    // The harness this host last started a worker on that took its brief.
    let delivered = world.cache().join("afkconvoy/watch/delivered.json");
    std::fs::create_dir_all(delivered.parent().unwrap()).unwrap();
    std::fs::write(delivered, r#"["codex"]"#).unwrap();
    run(&world, &dir);
    let first = until(&world, "a Codex watch", || running(&world));
    assert_eq!(first["harness"], "codex");
    assert!(!starts(&world)[0].contains(" exec "), "{:?}", starts(&world));
    assert!(starts(&world)[0].contains(" --search "), "the watch can search the web");

    let out = "Usage limit reached; increase your limits to continue using Codex";
    printed(&world, &first, &["checked kimi", out], None);
    let next = until(&world, "the watch on Claude Code", || {
        running(&world).filter(|running| running["harness"] == "claude-code")
    });
    assert_eq!((&next["id"], &next["started"]), (&first["id"], &first["started"]));
    assert_eq!(next["attempts"][0]["harness"], "codex");
    assert_eq!(next["attempts"][0]["screen"], json!(["checked kimi", out]));
    let started = starts(&world);
    assert_eq!(started.len(), 2, "{started:?}");
    let told = brief(&world, &started[1]);
    assert!(told.contains("checked kimi") && told.contains("`kimi` is paused"), "{told}");
    let live = open(&world, &dir);
    assert_eq!(live, [OPERATORS.to_string(), next["terminal"].as_str().unwrap().into()]);
    let log = world.runner_log().join("\n");
    assert!(log.contains("continued from codex to claude-code"), "{log}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    let faults = doctor["host"]["host_kit"]["worker_health"]["faults"].as_array().unwrap();
    assert!(
        faults.iter().any(|f| f["harness"] == "codex" && f["scope"] == "account"),
        "{faults:?}"
    );

    // A harness that fails its session, and says nothing of why, gives out too.
    printed(&world, &next, &["API Error: 500"], Some(1));
    let third = until(&world, "the watch on pi", || {
        running(&world).filter(|running| running["harness"] == "pi")
    });
    assert_eq!(third["attempts"][1]["harness"], "claude-code");
    assert_eq!(third["id"], first["id"]);
    printed(&world, &third, &["fixed it"], Some(0));
    let last = until(&world, "the watch to end", || ended(&world, "ended"));
    assert_eq!(last["did"], json!(["fixed it"]));
}

#[test]
fn a_watch_is_ended_at_its_hour_and_one_no_harness_can_start_is_a_finding() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "it refused every prompt");
    run(&world, &dir);
    let watch = until(&world, "a watch", || running(&world));
    let file = world.cache().join("afkconvoy/watch/state.json");
    let mut kept = state(&world);
    let hour_ago = kept["running"]["started"].as_u64().unwrap() - 3600;
    kept["running"]["started"] = json!(hour_ago);
    std::fs::write(&file, kept.to_string()).unwrap();
    let last = until(&world, "the watch to reach its hour", || ended(&world, "time_limit"));
    assert_eq!(last["id"], watch["id"]);
    assert_eq!(
        open(&world, &dir),
        [OPERATORS, watch["terminal"].as_str().unwrap()],
        "the terminal and operator stay open"
    );
    assert_eq!(last["report"], Value::Null);

    paused(&world, "claude-code", "out of tokens");
    let why = until(&world, "no harness to start one", || {
        state(&world)["unavailable"].as_str().map(String::from)
    });
    assert!(why.contains("no harness free of a fault"), "{why}");
    assert_eq!(starts(&world).len(), 1);
    let doctor = world.afk(&["doctor", "--all"]);
    assert_eq!(doctor.code, 1, "{}", doctor.json);
    let findings = doctor.json["findings"].as_array().unwrap();
    let finding = findings
        .iter()
        .find(|f| f["what"] == "no harness can start this host's watch")
        .expect("the kit that cannot start keeps that row");
    assert_eq!(finding["fix"], "afkconvoy kit");
    assert_eq!(doctor.json["host"]["watch"]["unavailable"], json!(why));
}

#[test]
fn a_busy_watch_is_that_watch_and_its_fix_is_one_command() {
    let world = World::new();
    let dir = operator(&world);
    world.list_checkout(&world.clone);
    let situation = "the resident watch is busy; its round remains undelivered. \
         No replacement is started and existing work is retained.";
    let watch_dir = world.cache().join("afkconvoy/watch");
    std::fs::create_dir_all(&watch_dir).unwrap();
    std::fs::write(watch_dir.join("state.json"), json!({"unavailable": situation}).to_string())
        .unwrap();
    std::fs::write(
        watch_dir.join("terminal.json"),
        json!({
            "watch": {"id": "round-1", "directory": dir, "look": {}, "items": [], "started": 0,
                "harness": "claude-code", "model": null, "terminal": "watch-term",
                "transcript": "/tmp/watch.log"},
            "status": "ready", "terminal": null, "command": "agent", "problem": situation
        })
        .to_string(),
    )
    .unwrap();

    let doctor = world.afk(&["doctor", "--all"]);
    let finding =
        finding_named(&doctor.json, "the resident watch is busy; its round remains undelivered");
    assert_eq!(finding["what"], "the resident watch is busy; its round remains undelivered");
    assert_eq!(finding["evidence"], situation);
    assert_eq!(doctor.json["host"]["watch"]["association"]["problem"], situation);
    assert!(doctor.json["host"]["host_kit"]["paused"].as_array().unwrap().is_empty());
    assert!(!finding["what"].as_str().unwrap().contains("no harness can start"), "{}", finding);
    let fix = finding["fix"].as_str().unwrap();
    assert!(!fix.contains(&world.clone.display().to_string()), "{fix}");
    assert!(fix.contains(&dir.display().to_string()), "{fix}");
    assert!(!fix.split_whitespace().any(|word| word == "--help"), "{fix}");
    assert!(!fix.contains(['(', ';', '&', '|', '\n']), "{fix}");
    let ran = world.command("sh", Path::new("/tmp")).arg("-c").arg(fix).output().unwrap();
    assert!(ran.status.success(), "{fix}: {}", String::from_utf8_lossy(&ran.stderr));
    let from = std::fs::canonicalize("/tmp").unwrap();
    let orca = world.orca();
    let recorded = orca["from"].as_array().unwrap().last().unwrap();
    assert_eq!(recorded, &json!(from.display().to_string()));
    let call = world.orca_calls().last().unwrap().clone();
    assert_eq!(asked(&call), "terminal list");
    assert!(call.iter().any(|arg| arg == &format!("path:{}", dir.display())));
    assert!(
        !call
            .iter()
            .any(|arg| { arg == "--help" || arg.contains(&world.clone.display().to_string()) })
    );

    world.pause("claude-code", 1, "it refused every prompt");
    let paused = world.afk(&["doctor", "--all"]);
    let blocked = finding_named(&paused.json, "no harness can start this host's watch");
    assert_eq!(blocked["fix"], "afkconvoy kit");
    assert!(!blocked["what"].as_str().unwrap().contains("the resident watch is busy"));

    let mut paused_items = world.paused();
    paused_items.as_array_mut().unwrap().clear();
    std::fs::write(world.paused_path(), paused_items.to_string()).unwrap();
    let key = command_key("agent");
    std::fs::write(
        world.cache().join("afkconvoy/worker-health.json"),
        json!({"workers": [],
            "faults": [{"harness": "claude-code", "command_key": key, "model": null,
                "scope": "account", "since": 1, "next_check": 9_999_999_999_u64,
                "evidence": ["a fault on record"]}],
            "observations_failed": []})
        .to_string(),
    )
    .unwrap();
    let faulted = world.afk(&["doctor", "--all"]);
    let blocked = finding_named(&faulted.json, "no harness can start this host's watch");
    assert_eq!(blocked["fix"], "afkconvoy kit");
    assert!(faulted.json["host"]["host_kit"]["paused"].as_array().unwrap().is_empty());
    assert!(
        !faulted.json["host"]["host_kit"]["worker_health"]["faults"].as_array().unwrap().is_empty()
    );
}

fn finding_named<'a>(doctor: &'a Value, what: &str) -> &'a Value {
    doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["what"].as_str().unwrap().contains(what))
        .unwrap_or_else(|| panic!("no finding {what}: {doctor}"))
}

fn command_key(command: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    command.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

#[test]
fn a_report_is_complete_durable_and_carried_into_the_next_brief() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    run(&world, &dir);
    let watch = until(&world, "watch", || running(&world));
    let told = brief(&world, &starts(&world)[0]);
    let command = told.lines().last().unwrap();
    assert!(command.starts_with("afkconvoy watched --watch "), "{told}");
    let file = world
        .cache()
        .join("afkconvoy/watch")
        .join(watch["id"].as_str().unwrap())
        .join("report.json");
    let report = json!({"held_back": ["pi could not authenticate"],
        "did": ["checked pi's login"], "verified": ["other fleets dispatched"],
        "human": [{"question": "Supply a new account?", "answer": "afkconvoy kit"}],
        "pending": ["pi still needs its account"]});
    std::fs::write(&file, report.to_string()).unwrap();
    let sent = world.command("sh", &dir).args(["-c", command]).output().unwrap();
    assert!(sent.status.success(), "{}", String::from_utf8_lossy(&sent.stdout));
    let last = until(&world, "report", || ended(&world, "ended"));
    assert_eq!(last["report"], report);
    assert!(open(&world, &dir).contains(&watch["terminal"].as_str().unwrap().to_string()));
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["last"]["reported"], true);
    assert_eq!(doctor["host"]["watch"]["last"]["report"], report);
    // An acknowledged report can be retried after the runner has ended it.
    world
        .afk(&[
            "watched",
            "--watch",
            watch["id"].as_str().unwrap(),
            "--text-file",
            file.to_str().unwrap(),
        ])
        .ok();
    std::fs::write(&file, json!({"did": []}).to_string()).unwrap();
    assert!(
        world
            .afk(&[
                "watched",
                "--watch",
                watch["id"].as_str().unwrap(),
                "--text-file",
                file.to_str().unwrap()
            ])
            .error()
            .contains("report JSON")
    );

    // A new watch on the same episode gets the complete report. Removing
    // coverage models the episode's recovery and return without a long wait.
    world.kill_host_runner("KILL");
    let mut kept = state(&world);
    kept["watched"] = json!([]);
    kept["covered"] = json!([]);
    kept["observed"] = json!({});
    std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
    run(&world, &dir);
    let next = until(&world, "next delivered round", || {
        running(&world)
            .filter(|w| w["id"] != watch["id"] && w["started"].as_u64().unwrap_or(0) != 0)
    });
    let told = brief(&world, starts(&world).last().unwrap());
    for text in ["Supply a new account?", "pi still needs its account", "other fleets dispatched"] {
        assert!(told.contains(text), "{told}");
    }
    assert_eq!(watch["terminal"], next["terminal"], "the next round reuses the resident Terminal");
    assert!(open(&world, &dir).contains(&next["terminal"].as_str().unwrap().to_string()));
    assert!(open(&world, &dir).contains(&OPERATORS.to_string()));
}

#[test]
fn printed_output_and_a_successful_exit_are_not_a_report() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    run(&world, &dir);
    let watch = until(&world, "watch", || running(&world));
    std::fs::write(
        watch["transcript"].as_str().unwrap(),
        format!("fixed everything\n{EXITED} 0\n"),
    )
    .unwrap();
    let last = until(&world, "not reported", || ended(&world, "not_reported"));
    assert_eq!(last["did"], json!([]));
    assert_eq!(last["report"], Value::Null);
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["watch"]["last"]["reported"], false);
    assert!(open(&world, &dir).contains(&watch["terminal"].as_str().unwrap().to_string()));
}

#[test]
fn a_stopped_watch_is_nudged_even_when_the_operators_other_session_is_working() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    run(&world, &dir);
    let watch = until(&world, "watch", || running(&world));
    world.kill_host_runner("KILL");
    let mut kept = state(&world);
    let since = watch["started"].as_u64().unwrap() - afkconvoy_core::worker::GRACE - 1;
    kept["running"]["session_started"] = json!(since);
    kept["running"]["started"] = json!(since);
    std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == watch["terminal"] {
                terminal["agent"] = json!({"state": "stopped", "stateStartedAt": since * 1000});
            } else {
                terminal["agent"] =
                    json!({"state": "working", "stateStartedAt": (since + 600) * 1000});
            }
        }
    });
    run(&world, &dir);
    until(&world, "nudge", || running(&world).filter(|watch| watch["nudged"].is_number()));
    let orca = world.orca();
    let own = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["handle"] == watch["terminal"])
        .unwrap();
    let sent = own["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 2, "brief, then nudge: {own}");
    assert!(sent[1]["text"].as_str().unwrap().contains("afkconvoy watched"));
    assert!(orca["terminals"].as_array().unwrap().iter().find(|t| t["handle"] == OPERATORS).unwrap()["sent"].as_array().unwrap().is_empty());
}

#[test]
fn a_watch_at_capacity_continues_interactively_with_the_same_look() {
    let world = World::new();
    let dir = operator(&world);
    world.with_kit(|kit| { kit["harnesses"] = json!([
        {"harness": "codex", "command": "codex --yolo", "models": []},
        {"harness": "claude-code", "command": "claude --dangerously-skip-permissions", "models": []}
    ]); });
    paused(&world, "pi", "a fault");
    run(&world, &dir);
    let watch = until(&world, "Codex watch", || running(&world));
    assert_eq!(watch["harness"], "codex");
    printed(
        &world,
        &watch,
        &["Selected model is at capacity. Please try a different model."],
        None,
    );
    let next = until(&world, "continued watch", || {
        running(&world).filter(|w| w["harness"] == "claude-code")
    });
    assert_eq!(next["look"], watch["look"]);
    assert_eq!(next["id"], watch["id"]);
    assert_eq!(next["started"], watch["started"]);
    assert!(brief(&world, starts(&world).last().unwrap()).contains("at capacity"));
    assert_eq!(next["did"], json!([]));
}

/// Fake process readings never count or signal processes of the test host.
fn resource_host(world: &World) {
    use std::os::unix::fs::PermissionsExt;
    let ps = world.root.join("bin/ps");
    std::fs::write(&ps, "#!/bin/sh\ncat \"${0%/*}/../host-processes\"\n").unwrap();
    std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        world.root.join("host-processes"),
        "999990 2 0.0 10485760 0:00.00 outside-memory-holder\n",
    )
    .unwrap();
    // The runner takes its PATH from the login profile. Keep a hostvitals
    // installed on the test host out of it; tests opt into a stand-in below.
    let path = world.login_env().into_iter().find(|(key, _)| key == "PATH").unwrap().1;
    let dirs = std::env::split_paths(&path).filter(|dir| !dir.join("hostvitals").exists());
    let path = std::env::join_paths(dirs).unwrap().to_string_lossy().replace('\'', "'\\''");
    let profile = world.home().join(".profile");
    let mut text = std::fs::read_to_string(&profile).unwrap();
    text.push_str(&format!("\nexport PATH='{path}'\n"));
    std::fs::write(profile, text).unwrap();
}

fn hostvitals(world: &World, answer: &str, status: i32) {
    use std::os::unix::fs::PermissionsExt;
    let file = world.root.join("bin/hostvitals");
    std::fs::write(&file, format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${{0%/*}}/../hostvitals-calls\"\ncat \"${{0%/*}}/../hostvitals-answer\"\nprintf 'stand-in diagnostic\\n' >&2\nexit {status}\n"
    )).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(world.root.join("hostvitals-answer"), answer).unwrap();
}

fn hostvitals_answer() -> Value {
    let process = json!({"pid": 42, "uid": 0, "name": "fseventsd",
        "resident_bytes": 10630044058_u64, "started_at": "2026-10-12T00:00:00Z"});
    json!({
        "findings": [{"rule": "process_resident", "subject": process,
            "measured": {"percent": 30}, "next_command": "hostvitals memory 42"}],
        "rankings": {"processes_by_memory": [process], "apps_by_memory": [{"app_pid": 12}]},
        "host": {"memory": {"pressure": "warning", "swap": {"used_bytes": 4096}}},
        "processes": [process], "apps": [{"app_pid": 12}], "skipped": []
    })
}

/// Read the evidence actually handed to the running watch in its look.
fn host_evidence(world: &World) -> String {
    let watch = running(world).unwrap();
    let resources = watch["look"]["host"]["resources"].as_array().unwrap();
    let file = resources.iter().find_map(|r| r["evidence"].as_str()).unwrap();
    std::fs::read_to_string(file).unwrap()
}

fn assert_hostvitals_evidence(world: &World) {
    let evidence = host_evidence(world);
    assert!(evidence.contains("Source: `hostvitals now`"), "{evidence}");
    assert!(!evidence.contains("The most memory resident"), "{evidence}");
    let start = evidence.find("```json\n{\n  \"findings\"").unwrap() + "```json\n".len();
    let end = start + evidence[start..].find("\n```").unwrap();
    let answer: Value = serde_json::from_str(&evidence[start..end]).unwrap();
    assert_eq!(answer, hostvitals_answer(), "all readings reach the watch");
    assert!(evidence[start..].find("\"findings\"") < evidence[start..].find("\"rankings\""));
    assert!(evidence[start..].find("\"rankings\"") < evidence[start..].find("\"apps\""));
    assert_eq!(std::fs::read_to_string(world.root.join("hostvitals-calls")).unwrap(), "now\n");
}

fn resource_worker(world: &World, issue: u64, mib: u64) -> PathBuf {
    world.issue(issue, &["ready-for-agent"]);
    let started =
        world.afk(&["dispatch", &issue.to_string(), "--instance", "alpha", "--host", "h1"]).ok();
    let path = PathBuf::from(started["worktree"].as_str().unwrap());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == json!(path) {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
            }
        }
    });
    let tmp = world.tmp(&path);
    std::fs::write(world.root.join("host-processes"), format!(
        "999990 2 0.0 10485760 0:00.00 outside-memory-holder\n999980 2 0.0 {} 0:00.00 worker TMPDIR={}\n",
        mib * 1024, tmp.display())).unwrap();
    path
}

#[test]
fn a_hot_worker_starts_the_host_watch_with_its_reading_and_evidence() {
    let world = World::new();
    resource_host(&world);
    hostvitals(&world, &hostvitals_answer().to_string(), 0);
    let path = resource_worker(&world, 7, 1536);
    world.say_host_settings(json!({"hot_memory": 1}));
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "a watch on the hot worker", || {
        running(&world).filter(|w| {
            w["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["id"].as_str().unwrap().starts_with("resource:hot:"))
        })
    });
    let told = brief(&world, &starts(&world)[0]);
    for text in ["hot worker", "#7", "resident", path.to_str().unwrap(), "evidence.md", "terminal"]
    {
        assert!(told.contains(text), "missing {text}: {told}");
    }
    assert!(
        !world.root.join("hostvitals-calls").exists(),
        "hot-worker sampling stays the fleet's own"
    );
    assert!(
        !world.worktrees().iter().any(|p| p.to_string_lossy().contains("afkconvoy-inspection"))
    );
    printed(&world, &watch, &["told #7 to end its runaway process"], Some(0));
    until(&world, "watch record", || ended(&world, "ended"));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(starts(&world).len(), 1, "the same hot episode starts nothing again");
    world.kill_host_runner("KILL");
    run(&world, &dir);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(starts(&world).len(), 1, "a runner replacement keeps the hot episode");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert!(
        doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["what"].as_str().unwrap().contains("hot worker"))
    );
}

/// Seed an uninterrupted observation ten minutes ago, through the same
/// durable episode format the runner reads after replacement.
fn held_since(world: &World, pressure: bool) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut holding = afkconvoy_core::resources::Holding::default();
    holding.ticked(
        now - 600,
        &[if pressure {
            afkconvoy_core::resources::Reason::Memory
        } else {
            afkconvoy_core::resources::Reason::Load
        }],
    );
    let mut state = json!({"admission": {}, "pressure": {}, "hot": {}});
    state["pressure"] =
        serde_json::to_value(afkconvoy_core::resources::Holding::default()).unwrap();
    if pressure {
        state["pressure"] = json!(holding);
    } else {
        world.issue(430, &["ready-for-agent"]);
        state["admission"][world.clone.to_str().unwrap()] = json!({"holding": holding, "stages": [],
                "waiting": [{"do": "dispatch", "issue": 430}]});
    }
    let file = world.cache().join("afkconvoy/watch/resources.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, state.to_string()).unwrap();
}

#[test]
fn admission_held_ten_minutes_starts_a_watch_with_the_host_reading() {
    let world = World::new();
    resource_host(&world);
    world.busy("1000.00");
    held_since(&world, false);
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "admission's watch", || running(&world));
    assert!(watch["items"].to_string().contains("resource:admission:"), "{watch}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    let findings = doctor["findings"].as_array().unwrap();
    let admission: Vec<_> = findings.iter().filter(|f| f["kind"] == "admission_held").collect();
    assert_eq!(admission.len(), 1, "{doctor}");
    assert!(admission[0].to_string().contains("430"), "{doctor}");
    let told = brief(&world, &starts(&world)[0]);
    for text in ["admission has held back", "load", "600", "reading", "cores", "evidence.md"] {
        assert!(told.contains(text), "missing {text}: {told}");
    }
    let evidence = host_evidence(&world);
    for text in [
        "Source: afkconvoy",
        "The most CPU taken",
        "The most memory resident",
        "outside-memory-holder",
    ] {
        assert!(evidence.contains(text), "{evidence}");
    }
    printed(&world, &watch, &["moved the heavy work through exclusive"], Some(0));
    until(&world, "watch end", || ended(&world, "ended"));
    let second = world.second_repo();
    world.afk_in(&second, &["run", "--host", "h1", "--second-ms", "100"]).ok();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(starts(&world).len(), 1, "a new fleet shares the host's admission episode");
    world.kill_host_runner("KILL");
    run(&world, &dir);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(starts(&world).len(), 1, "episode coverage survives a runner replacement");
}

#[test]
fn idle_checkout_does_not_inherit_an_old_host_hold_when_work_appears() {
    let world = World::new();
    resource_host(&world);
    world.busy("1000.00");
    held_since(&world, false);
    // An older runner recorded a host hold for an idle checkout.
    let file = world.cache().join("afkconvoy/watch/resources.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    saved["admission"][world.clone.to_str().unwrap()]["waiting"] = json!([]);
    std::fs::write(&file, saved.to_string()).unwrap();
    world.close_issue(430);
    let dir = operator(&world);
    run(&world, &dir);
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert!(!doctor["findings"].to_string().contains("admission_held"), "{doctor}");
    assert!(starts(&world).is_empty());
    world.with_gh(|gh| gh["issues"][0]["state"] = json!("open"));
    until(&world, "waiting issue recorded", || {
        let saved: Value = serde_json::from_slice(&std::fs::read(&file).ok()?).ok()?;
        (!saved["admission"][world.clone.to_str()?]["waiting"].as_array()?.is_empty()).then_some(())
    });
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert!(!doctor["findings"].to_string().contains("admission_held"), "{doctor}");
}

#[test]
fn a_load_watch_receives_hostvitals_findings_then_rankings() {
    let world = World::new();
    resource_host(&world);
    hostvitals(&world, &hostvitals_answer().to_string(), 0);
    world.busy("1000.00");
    held_since(&world, false);
    let dir = operator(&world);
    run(&world, &dir);
    until(&world, "load's watch", || running(&world));
    assert_hostvitals_evidence(&world);
}

#[test]
fn a_failed_or_unrecognisable_hostvitals_answer_keeps_the_fleets_evidence() {
    for (answer, status) in [
        ("failed to read host\ntry hostvitals doctor", 1),
        (r#"{"error":"host reading unavailable"}"#, 0),
        ("not JSON", 0),
        (r#"{"findings":[],"rankings":{},"error":"partial failure"}"#, 0),
        (r#"{"findings":[]}"#, 0),
    ] {
        let world = World::new();
        resource_host(&world);
        hostvitals(&world, answer, status);
        world.busy("1000.00");
        held_since(&world, false);
        let dir = operator(&world);
        run(&world, &dir);
        until(&world, "watch despite hostvitals failure", || running(&world));
        let evidence = host_evidence(&world);
        let source = evidence.lines().find(|line| line.starts_with("Source:")).unwrap();
        assert!(source.contains("Source: afkconvoy"), "{evidence}");
        assert!(
            source.contains(&answer.split_whitespace().collect::<Vec<_>>().join(" ")),
            "{evidence}"
        );
        assert!(source.contains("stand-in diagnostic"), "{evidence}");
        for text in ["The most CPU taken", "The most memory resident", "outside-memory-holder"] {
            assert!(evidence.contains(text), "{evidence}");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn pressure_starts_one_watch_across_fleets_and_no_worker_turns_hot() {
    let world = World::new();
    resource_host(&world);
    hostvitals(&world, &hostvitals_answer().to_string(), 0);
    let path = resource_worker(&world, 7, 620);
    std::fs::write(world.root.join("host-pressure"), "2").unwrap();
    held_since(&world, true);
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "pressure's watch", || running(&world));
    assert!(watch["items"].to_string().contains("resource:pressure:"), "{watch}");
    let told = brief(&world, &starts(&world)[0]);
    assert!(told.contains("under memory pressure"), "{told}");
    assert!(told.contains("level"), "{told}");
    assert_hostvitals_evidence(&world);
    let evidence = host_evidence(&world);
    assert!(evidence.contains(&format!("### `{}` (issue #7)", path.display())), "{evidence}");
    assert!(evidence.contains("worker TMPDIR="), "{evidence}");
    let second = world.second_repo();
    world.afk_in(&second, &["run", "--host", "h1", "--second-ms", "100"]).ok();
    printed(&world, &watch, &["checked the outside memory holder"], Some(0));
    until(&world, "watch end", || ended(&world, "ended"));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(starts(&world).len(), 1, "one pressure episode across fleets");
    let doctor = world.afk(&["doctor", "--all"]).json;
    for checkout in doctor["checkouts"].as_array().unwrap() {
        assert_eq!(checkout["doctor"]["fleet"]["hot_workers"], json!([]));
    }
}

/// The stand-in process table as `ps` says it with uids: this account's
/// processes holding `mine` GiB, and the other account `other` — uid, user,
/// GiB — its own. Every other `ps` is the host's stand-in table.
fn account_table(world: &World, mine: u64, other: (u32, &str, u64)) {
    use std::os::unix::fs::PermissionsExt;
    let ps = world.root.join("bin/ps");
    std::fs::write(
        &ps,
        "#!/bin/sh\ncase \"$*\" in\n*uid=*) cat \"${0%/*}/../account-processes\" ;;\n\
         *) cat \"${0%/*}/../host-processes\" ;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
    let me = std::process::Command::new("id").arg("-u").output().unwrap();
    let me = String::from_utf8(me.stdout).unwrap();
    let (uid, user, gib) = other;
    let kib = |gib: u64| gib << 20;
    std::fs::write(
        world.root.join("account-processes"),
        format!(
            "{} me 100 1.0 {} /bin/zsh\n{uid} {user} 200 80.0 {} /opt/colima/limactl hostagent\n\
             {uid} {user} 300 20.0 {} launchd_sim\n0 root 1 0.1 {} /sbin/launchd\n",
            me.trim(),
            kib(mine),
            kib(gib) - kib(1),
            kib(1),
            kib(40),
        ),
    )
    .unwrap();
}

/// Pressure held for the span, and this host's fleets' samplers watching
/// `workers` of its checkout's worktrees, as the runner's record says them.
/// What a watch left with the operator, already recorded, is kept.
fn pressed_with_workers(world: &World, since: u64, workers: &[&Path]) {
    let file = world.cache().join("afkconvoy/watch/resources.json");
    let left = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map_or(Value::Null, |saved| saved["left"].clone());
    held_since(world, true);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    saved["left"] = left;
    let mut holding = afkconvoy_core::resources::Holding::default();
    holding.ticked(since, &[afkconvoy_core::resources::Reason::Memory]);
    saved["pressure"] = json!(holding);
    let watched: serde_json::Map<String, Value> = workers
        .iter()
        .map(|path| {
            let watch = afkconvoy_core::hot::Watch::default();
            (path.to_str().unwrap().to_string(), serde_json::to_value(watch).unwrap())
        })
        .collect();
    saved["samples"] = json!({world.clone.to_str().unwrap(): watched});
    std::fs::write(file, saved.to_string()).unwrap();
}

fn pressure_finding(doctor: &Value) -> Value {
    let findings = doctor["findings"].as_array().unwrap();
    let pressure = findings
        .iter()
        .filter(|f| f["kind"] == "memory_pressure" || f["kind"] == "pressure_elsewhere");
    let pressure: Vec<_> = pressure.collect();
    assert_eq!(pressure.len(), 1, "{doctor}");
    pressure[0].clone()
}

/// Every watch round of this host so far, by id, oldest first: a resident
/// watch takes a later round in the same session, with no new terminal.
fn rounds(world: &World) -> Vec<String> {
    let state = state(world);
    let mut ids: Vec<String> = vec![];
    let history = state["history"].as_array().cloned().unwrap_or_default();
    for round in history.iter().chain([&state["last"], &state["running"]]) {
        if let Some(id) = round["id"].as_str()
            && !ids.iter().any(|seen| seen == id)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

#[cfg(target_os = "macos")]
#[test]
fn pressure_another_account_holds_with_no_fleet_worker_starts_no_watch_and_doctor_names_it() {
    let world = World::new();
    resource_host(&world);
    account_table(&world, 1, (777, "simulators", 20));
    std::fs::write(world.root.join("host-pressure"), "2").unwrap();
    held_since(&world, true);
    let dir = operator(&world);
    run(&world, &dir);
    let first = until(&world, "a look", || state(&world)["looked"].as_u64());
    until(&world, "a later look", || state(&world)["looked"].as_u64().filter(|at| *at > first));
    std::thread::sleep(Duration::from_secs(2));
    assert!(starts(&world).is_empty(), "no watch: {}", state(&world));
    let doctor = world.afk(&["doctor", "--all"]).json;
    let found = pressure_finding(&doctor);
    assert_eq!(found["kind"], "pressure_elsewhere", "{found}");
    assert_eq!(found["severity"], "info", "{found}");
    let what = found["what"].as_str().unwrap();
    for text in ["from another account", "simulators (uid 777)", "20.0 GiB", "no worker"] {
        assert!(what.contains(text), "missing {text}: {what}");
    }
    let resources = doctor["host"]["resources"].as_array().unwrap();
    let pressure = resources.iter().find(|r| r["kind"] == "pressure").unwrap();
    assert_eq!(pressure["elsewhere"]["account"], "simulators", "{pressure}");
    assert_eq!(pressure["unwatched"], "no_worker", "{pressure}");
    let plan = world.afk(&["plan"]).json;
    assert_eq!(plan["memory_pressure"]["account"], "simulators", "{plan}");
    assert_eq!(plan["memory_pressure"]["unwatched"], "no_worker", "{plan}");
}

#[cfg(target_os = "macos")]
#[test]
fn the_same_pressure_while_this_accounts_fleet_holds_the_most_starts_a_watch() {
    let world = World::new();
    resource_host(&world);
    let path = resource_worker(&world, 7, 620);
    account_table(&world, 20, (777, "simulators", 2));
    std::fs::write(world.root.join("host-pressure"), "2").unwrap();
    pressed_with_workers(&world, now() - 600, &[&path]);
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "pressure's watch", || running(&world));
    assert!(watch["items"].to_string().contains("resource:pressure:"), "{watch}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(pressure_finding(&doctor)["kind"], "memory_pressure", "{doctor}");
}

#[cfg(target_os = "macos")]
#[test]
fn two_pressure_episodes_from_the_same_other_account_start_one_watch_until_the_cause_changes() {
    let world = World::new();
    resource_host(&world);
    let path = resource_worker(&world, 7, 620);
    account_table(&world, 1, (777, "simulators", 20));
    std::fs::write(world.root.join("host-pressure"), "2").unwrap();
    pressed_with_workers(&world, now() - 600, &[&path]);
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "the first episode's watch", || running(&world));
    assert!(watch["items"].to_string().contains("resource:pressure:"), "{watch}");
    let first = watch["id"].as_str().unwrap().to_string();
    printed(&world, &watch, &["left the other account's memory with the operator"], Some(0));
    until(&world, "watch end", || ended(&world, "ended"));
    let file = world.cache().join("afkconvoy/watch/resources.json");
    let saved = until(&world, "pressure left with the operator", || {
        let saved: Value = serde_json::from_slice(&std::fs::read(&file).ok()?).ok()?;
        (saved["left"]["account"] == "simulators").then_some(saved)
    });
    assert_eq!(saved["left"]["uid"], 777, "{saved}");

    // Pressure dips for five minutes and rises again: a new episode, with
    // the same account holding the most.
    world.kill_host_runner("KILL");
    pressed_with_workers(&world, now() - 700, &[&path]);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["left"]["account"], "simulators", "{saved}");
    run(&world, &dir);
    let looked = now();
    until(&world, "a look of the new episode", || {
        state(&world)["looked"].as_u64().filter(|at| *at > looked)
    });
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(rounds(&world), vec![first.clone()], "the same cause starts no second watch");
    let doctor = world.afk(&["doctor", "--all"]).json;
    let found = pressure_finding(&doctor);
    assert_eq!(found["kind"], "pressure_elsewhere", "{found}");
    assert!(found["what"].as_str().unwrap().contains("left it with the operator"), "{found}");

    // Another account holds the most now: the cause changed.
    account_table(&world, 1, (778, "builder", 20));
    let watch = until(&world, "a watch round on the changed cause", || {
        running(&world).filter(|round| round["id"] != first.as_str())
    });
    assert!(watch["items"].to_string().contains("builder"), "{watch}");
    saved = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(saved["left"].is_null(), "{saved}");
}

#[test]
fn a_resource_trigger_while_a_watch_runs_waits_for_its_next_look() {
    let world = World::new();
    resource_host(&world);
    paused(&world, "pi", "it refused every prompt");
    let dir = operator(&world);
    run(&world, &dir);
    let first = until(&world, "first watch", || running(&world));
    world.say_host_settings(json!({"hot_memory": 1, "watch_dir": dir, "watch_every": 1}));
    resource_worker(&world, 7, 1536);
    until(&world, "hot reading", || {
        let file = world.cache().join("afkconvoy/watch/resources.json");
        let saved = std::fs::read_to_string(file).unwrap_or_default();
        saved.contains("resident").then_some(())
    });
    assert_eq!(starts(&world).len(), 1, "no second watch while the first runs");
    until(&world, "resource episode queued during the first round", || {
        state(&world)["pending"]
            .as_array()?
            .iter()
            .any(|q| q["item"]["id"].as_str().is_some_and(|id| id.starts_with("resource:hot:")))
            .then_some(())
    });
    printed(&world, &first, &["checked pi"], Some(0));
    let next = until(&world, "watch on the new resource episode", || {
        running(&world).filter(|w| w["id"] != first["id"])
    });
    assert!(next["items"].to_string().contains("resource:hot:"), "{next}");
    assert_eq!(starts(&world).len(), 1);
    assert_eq!(next["terminal"], first["terminal"]);
    assert_eq!(next["session"], first["session"]);
}

#[test]
fn the_removed_reporting_command_is_not_in_the_manual_or_command_tree() {
    let world = World::new();
    assert!(world.afk(&["inspected", "--text", "x"]).error().contains("unrecognized subcommand"));
    let help = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(!help.contains("inspection"));
    assert!(!help.contains("inspected"));
}

#[test]
fn a_lost_creation_receipt_recovers_the_acknowledged_terminal_without_another_agent() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    // The terminal exists and ran its startup acknowledgement, but the
    // caller receives neither valid JSON nor a creation handle.
    world.with_orca(|orca| orca["raw_answers"]["terminal create"] = json!("lost receipt"));
    run(&world, &dir);
    let watch = until(&world, "recovered creation", || {
        running(&world).filter(|w| !w["terminal"].as_str().unwrap_or("").is_empty())
    });
    assert_eq!(starts(&world).len(), 1);
    let handle = watch["terminal"].as_str().unwrap();
    assert!(open(&world, &dir).contains(&handle.into()));
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["watch"]["association"]["status"], "running");
    assert_eq!(doctor["host"]["watch"]["association"]["terminal"], handle);

    // Lose both the caller's saved ID and its progress file after a hard
    // runner restart. The launch journal still owns this exact session.
    world.kill_host_runner("KILL");
    std::fs::remove_file(world.cache().join("afkconvoy/watch/state.json")).unwrap();
    run(&world, &dir);
    until(&world, "the retained watch", || running(&world).filter(|w| w["terminal"] == handle));
    std::thread::sleep(Duration::from_millis(2200));
    assert_eq!(starts(&world).len(), 1, "restart/repeated checks never create another agent");
    let terminal = world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["handle"] == handle)
        .unwrap()
        .clone();
    assert_eq!(terminal["sent"].as_array().unwrap().len(), 1, "brief delivered once");
    assert_eq!(running(&world).unwrap()["id"], watch["id"]);
    assert!(open(&world, &dir).contains(&OPERATORS.into()));
}

#[test]
fn same_name_operator_terminals_block_discovery_without_being_adopted_or_closed() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    world.with_orca(|orca| {
        orca["terminals"][0]["title"] = json!("afkconvoy watch");
        orca["terminals"].as_array_mut().unwrap().push(json!({"handle": "other",
            "worktreePath": dir, "title": "afkconvoy watch", "open": true, "sent": []}));
    });
    run(&world, &dir);
    let why = until(&world, "ambiguous discovery", || {
        state(&world)["unavailable"].as_str().map(str::to_string)
    });
    assert!(why.contains("unverified terminals"), "{why}");
    assert!(starts(&world).is_empty());
    assert_eq!(open(&world, &dir).len(), 2);
    // A unique title still proves no ownership.
    world.with_orca(|orca| orca["terminals"][1]["title"] = json!("operator's work"));
    std::thread::sleep(Duration::from_millis(2200));
    assert!(starts(&world).is_empty());
    world.with_orca(|orca| orca["terminals"][0]["title"] = json!("operator"));
    until(&world, "start after unrelated titles are renamed", || running(&world));
    assert_eq!(starts(&world).len(), 1);
    for handle in [OPERATORS, "other"] {
        let terminal = world.orca()["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["handle"] == handle)
            .unwrap()
            .clone();
        assert_eq!(terminal["open"], true);
        assert!(terminal["sent"].as_array().unwrap().is_empty());
    }
}

#[test]
fn stale_runtime_handles_reassociate_by_confirmed_pane_identity_and_keep_interactive_input() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    run(&world, &dir);
    let watch = until(&world, "watch", || running(&world));
    world.kill_host_runner("KILL");
    let old = watch["terminal"].as_str().unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == old {
                terminal["ptyId"] = json!(old);
                terminal["handle"] = json!("new-runtime-handle");
                terminal["screen"] = json!(["unfinished work", "human input retained"]);
                terminal["title"] = json!("agent changed its title");
            }
        }
        // An unrelated session with the fixed title cannot displace the
        // exact saved identity, even after a runtime handle changes.
        orca["terminals"][0]["title"] = json!("afkconvoy watch");
    });
    // Multiple discovery candidates are deliberately not guessed between.
    run(&world, &dir);
    until(&world, "ambiguous recovery", || {
        state(&world)["unavailable"]
            .as_str()
            .filter(|why| why.contains("multiple watch terminal candidates"))
            .map(str::to_string)
    });
    assert_eq!(starts(&world).len(), 1);
    world.with_orca(|orca| orca["terminals"][0]["title"] = json!("operator"));
    until(&world, "reassociated pane", || {
        running(&world).filter(|w| w["terminal"] == "new-runtime-handle")
    });
    assert_eq!(starts(&world).len(), 1);
    let terminal = world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["handle"] == "new-runtime-handle")
        .unwrap()
        .clone();
    assert_eq!(terminal["screen"], json!(["unfinished work", "human input retained"]));
    assert_eq!(terminal["sent"].as_array().unwrap().len(), 1);
}

/// Orca restored the resident's pane after a restart: same ptyId, a new
/// handle and a new incarnation, still titled `afkconvoy watch`
/// (sunfmin/control#807). The resident is ended and its pane closed, and
/// the next look starts a fresh watch instead of staying unavailable.
#[test]
fn a_resident_pane_orca_restores_under_a_new_incarnation_is_replaced_by_a_fresh_watch() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "first round");
    run(&world, &dir);
    let first = until(&world, "first round", || running(&world));
    printed(&world, &first, &["reported before the restart"], Some(0));
    until(&world, "first report", || ended(&world, "ended"));
    world.kill_host_runner("KILL");
    let old = first["terminal"].as_str().unwrap().to_string();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == old {
                terminal["ptyId"] = json!(old);
                terminal["handle"] = json!("restored-handle");
                terminal["incarnationId"] = json!("restored-incarnation");
            }
        }
    });
    paused(&world, "dsh", "a finding after the restart");
    run(&world, &dir);
    let next = until(&world, "a fresh watch", || {
        running(&world).filter(|w| w["terminal"] != old && w["terminal"] != "restored-handle")
    });
    assert_ne!(next["session"], first["session"]);
    assert_eq!(starts(&world).len(), 2, "one fresh agent, after the old one ended");
    assert!(!open(&world, &dir).contains(&"restored-handle".into()), "the restored pane closed");
    assert!(open(&world, &dir).contains(&OPERATORS.into()));
    let restored = world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["handle"] == "restored-handle")
        .unwrap()
        .clone();
    assert_eq!(restored["sent"].as_array().unwrap().len(), 1, "only the first brief, nothing more");
    assert!(state(&world)["unavailable"].is_null());
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert!(!doctor.to_string().contains("watch-unavailable"), "{doctor}");
    assert_eq!(doctor["host"]["watch"]["association"]["terminal"], next["terminal"]);
}

#[test]
fn uncertain_creation_and_prompt_receipts_pause_across_restarts_and_accept_an_explicit_report() {
    for status in ["creating", "sending"] {
        let world = World::new();
        let dir = operator(&world);
        paused(&world, "pi", "a fault");
        run(&world, &dir);
        let watch = until(&world, "watch", || running(&world));
        world.kill_host_runner("KILL");
        let file = world.cache().join("afkconvoy/watch/terminal.json");
        let mut record: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        record["status"] = json!(status);
        // A retained older runner had no caller-allocated delivery key.
        // Such an ambiguous send cannot be replayed safely.
        record["request"] = Value::Null;
        if status == "creating" {
            record["terminal"] = Value::Null;
            record["watch"]["terminal"] = json!("");
            let transcript = Path::new(watch["transcript"].as_str().unwrap());
            std::fs::remove_file(transcript.with_extension("terminal")).unwrap();
            let mut state = state(&world);
            state["running"]["terminal"] = json!("");
            std::fs::write(world.cache().join("afkconvoy/watch/state.json"), state.to_string())
                .unwrap();
        }
        std::fs::write(&file, record.to_string()).unwrap();
        run(&world, &dir);
        until(&world, "explicit recovery finding", || {
            state(&world)["unavailable"]
                .as_str()
                .filter(|why| why.contains("No replacement is started"))
                .map(str::to_string)
        });
        std::thread::sleep(Duration::from_millis(2200));
        assert_eq!(starts(&world).len(), 1, "{status}");
        assert!(open(&world, &dir).contains(&watch["terminal"].as_str().unwrap().into()));
        let doctor = world.afk(&["doctor"]).json;
        assert!(
            doctor["host"]["watch"]["unavailable"].as_str().unwrap().contains("afkconvoy watched")
        );
        printed(&world, &watch, &["inspected and retained the existing work"], Some(0));
        until(&world, "explicit report settles uncertainty", || ended(&world, "ended"));
    }
}

fn sent(world: &World, watch: &Value) -> Vec<Value> {
    world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["handle"] == watch["terminal"])
        .unwrap()["sent"]
        .as_array()
        .unwrap()
        .clone()
}

fn busy(world: &World, watch: &Value, working: bool) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == watch["terminal"] {
                terminal["agent"] = json!({"state": if working { "working" } else { "stopped" },
                    "stateStartedAt": 1000});
            }
        }
    });
}

#[test]
fn resident_rounds_queue_while_busy_revalidate_recover_and_keep_reports_separate() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "first episode");
    run(&world, &dir);
    let first = until(&world, "first round", || running(&world));
    busy(&world, &first, true);
    let before = state(&world)["until"].as_u64().unwrap();
    paused(&world, "dsh", "a queued condition that will recover");
    paused(&world, "kimi", "second round");
    let queued = until(&world, "two episodes queued during execution", || {
        let s = state(&world);
        (s["pending"].as_array()?.len() == 2 && s["until"].as_u64()? > before).then_some(s)
    });
    let episodes = queued["pending"].clone();
    until(&world, "another busy look", || {
        state(&world)["until"].as_u64().filter(|at| *at > queued["until"].as_u64().unwrap())
    });
    assert_eq!(state(&world)["pending"][0]["episode"], episodes[0]["episode"]);
    assert_eq!(state(&world)["pending"][1]["episode"], episodes[1]["episode"]);
    assert_eq!(sent(&world, &first).len(), 1, "busy input remains untouched");
    let mut paused_items = world.paused();
    paused_items.as_array_mut().unwrap().retain(|p| p["harness"] != "dsh");
    std::fs::write(world.paused_path(), paused_items.to_string()).unwrap();
    until(&world, "recovered episode removed before delivery", || {
        let s = state(&world);
        (s["pending"].as_array()?.len() == 1
            && s["pending"][0]["item"]["id"].as_str()?.contains("kimi"))
        .then_some(())
    });
    world.kill_host_runner("KILL");
    run(&world, &dir);
    assert_eq!(running(&world).unwrap()["id"], first["id"]);
    assert_eq!(starts(&world).len(), 1);
    printed(&world, &first, &["first round reported"], Some(0));
    until(&world, "first report", || ended(&world, "ended"));
    let at = state(&world)["until"].as_u64().unwrap();
    until(&world, "a look after the report while input stays busy", || {
        state(&world)["until"].as_u64().filter(|until| *until > at)
    });
    assert!(running(&world).is_none());
    assert_eq!(sent(&world, &first).len(), 1);
    busy(&world, &first, false);
    let second = until(&world, "second round on the resident", || {
        running(&world).filter(|w| w["id"] != first["id"] && w["started"].as_u64().unwrap_or(0) > 0)
    });
    assert_eq!(second["terminal"], first["terminal"]);
    assert_eq!(second["session"], first["session"]);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["items"][0]["id"].as_str().unwrap().contains("kimi"));
    assert_eq!(starts(&world).len(), 1);
    assert_eq!(sent(&world, &second).len(), 2);
    // A repeated first report cannot complete the second round.
    let first_report = world
        .cache()
        .join("afkconvoy/watch")
        .join(first["id"].as_str().unwrap())
        .join("reported.json");
    world
        .afk(&[
            "watched",
            "--watch",
            first["id"].as_str().unwrap(),
            "--text-file",
            first_report.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(running(&world).unwrap()["id"], second["id"]);
    assert!(
        world
            .afk(&[
                "watched",
                "--watch",
                "wrong-round",
                "--text-file",
                first_report.to_str().unwrap()
            ])
            .error()
            .contains("not the running watch")
    );
    printed(&world, &second, &["second round reported"], Some(0));
    until(&world, "second independent report", || {
        ended(&world, "ended").filter(|w| w["id"] == second["id"])
    });
    let at = state(&world)["until"].as_u64().unwrap();
    until(&world, "idle look", || state(&world)["until"].as_u64().filter(|until| *until > at));
    assert_eq!(sent(&world, &second).len(), 2, "standby never nudges or calls a model");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["session"]["status"], "standby");
    assert_eq!(doctor["host"]["watch"]["last"]["id"], second["id"]);
    assert!(doctor["host"]["watch"].get("rounds").is_none());
    let with_rounds = world.afk(&["doctor", "--all", "--rounds"]).json;
    assert_eq!(with_rounds["host"]["watch"]["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(doctor["host"]["watch"]["pending"], json!([]));
    assert!(open(&world, &dir).contains(&OPERATORS.into()));
    // A real observed recovery ends coverage; its return is a new episode.
    let mut paused_items = world.paused();
    paused_items.as_array_mut().unwrap().retain(|p| p["harness"] != "pi");
    std::fs::write(world.paused_path(), paused_items.to_string()).unwrap();
    until(&world, "pi's observed recovery", || {
        (!state(&world)["observed"].as_object()?.keys().any(|id| id.contains(r#""pi""#)))
            .then_some(())
    });
    paused(&world, "pi", "a new episode");
    let third = until(&world, "the new episode", || running(&world));
    assert_eq!(third["terminal"], first["terminal"]);
    assert_ne!(third["episodes"], first["episodes"]);
    printed(&world, &third, &["new episode reported"], Some(0));
    until(&world, "third independent report", || {
        ended(&world, "ended").filter(|w| w["id"] == third["id"])
    });
}

#[test]
fn standby_and_completed_round_age_do_not_spend_the_next_rounds_hour() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "first round");
    run(&world, &dir);
    let first = until(&world, "first round", || running(&world));
    printed(&world, &first, &["reported before standby"], Some(0));
    until(&world, "first report", || ended(&world, "ended"));
    world.kill_host_runner("KILL");
    let mut kept = state(&world);
    let old = first["started"].as_u64().unwrap() - 7200;
    kept["resident"]["started"] = json!(old);
    kept["resident"]["session_started"] = json!(old);
    std::fs::write(world.cache().join("afkconvoy/watch/state.json"), kept.to_string()).unwrap();
    let file = world.cache().join("afkconvoy/watch/terminal.json");
    let mut record: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    record["watch"]["started"] = json!(old);
    record["watch"]["session_started"] = json!(old);
    std::fs::write(file, record.to_string()).unwrap();
    let before = kept["until"].as_u64().unwrap();
    run(&world, &dir);
    until(&world, "an idle look after two hours", || {
        state(&world)["until"].as_u64().filter(|at| *at > before)
    });
    assert!(running(&world).is_none());
    assert_eq!(sent(&world, &first).len(), 1, "no idle nudge or model turn");
    paused(&world, "dsh", "next round");
    let next = until(&world, "a fresh execution hour", || {
        running(&world).filter(|w| w["started"].as_u64().unwrap_or(0) > before)
    });
    assert_eq!(next["terminal"], first["terminal"]);
    assert_eq!(next["session"], first["session"]);
    assert_eq!(next["session_started"], json!(old), "the executor is older than this round");
    assert!(!afkconvoy_core::host_watch::expired(next["started"].as_u64().unwrap(), before + 10));
    printed(&world, &next, &["fresh round reported"], Some(0));
    until(&world, "fresh report", || ended(&world, "ended").filter(|w| w["id"] == next["id"]));
}

#[test]
fn a_second_rounds_lost_send_receipt_replays_after_restart_without_typing_twice() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "first round");
    run(&world, &dir);
    let first = until(&world, "first round", || running(&world));
    printed(&world, &first, &["first done"], Some(0));
    until(&world, "first report", || ended(&world, "ended"));
    world.with_orca(|orca| {
        orca["raw_answers"]["terminal send"] = json!("lost response after accepting input")
    });
    paused(&world, "dsh", "second round");
    until(&world, "retained uncertain second delivery", || {
        state(&world)["unavailable"].as_str().map(str::to_string)
    });
    world.kill_host_runner("KILL");
    let file = world.cache().join("afkconvoy/watch/terminal.json");
    let intent: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(intent["status"], "sending");
    assert!(intent["request"].is_string());
    assert_ne!(intent["watch"]["id"], first["id"]);
    assert_eq!(intent["watch"]["started"], 0, "unproven execution consumes no hour");
    assert_eq!(sent(&world, &first).len(), 2);
    world.with_orca(|orca| orca["raw_answers"] = json!({}));
    run(&world, &dir);
    until(&world, "replayed exact submission", || {
        let receipt: Value = serde_json::from_slice(&std::fs::read(&file).ok()?).ok()?;
        (receipt["status"] == "running" && receipt["receipt"]["mutation"]["replayed"] == true)
            .then_some(receipt)
    });
    let second = running(&world).unwrap();
    assert_eq!(second["id"], intent["watch"]["id"]);
    assert_eq!(second["session"], first["session"]);
    assert_eq!(
        sent(&world, &second).len(),
        2,
        "one input per round across a lost receipt and restart"
    );
    assert_eq!(starts(&world).len(), 1);
    printed(&world, &second, &["recovered round reported"], Some(0));
    until(&world, "recovered report", || {
        ended(&world, "ended").filter(|w| w["id"] == second["id"])
    });
}

#[test]
fn an_unobservable_provider_needs_working_evidence_and_reuses_its_accepted_request() {
    let world = World::new();
    let dir = operator(&world);
    world.with_orca(|orca| {
        orca["prompt_stages"] = json!(["input_accepted"]);
        orca["prompt_observation"] = json!("unsupported");
        orca["send_screen"] = json!(["idle prompt; no new turn proven"]);
    });
    paused(&world, "pi", "first round");
    run(&world, &dir);
    until(&world, "accepted input without execution evidence", || {
        state(&world)["unavailable"].as_str().map(str::to_string)
    });
    let waiting = running(&world).unwrap();
    assert_eq!(waiting["started"], 0, "input acceptance alone consumes no execution hour");
    assert_eq!(sent(&world, &waiting).len(), 1);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    afkconvoy::session::record(&world.home(), &dir, true, now).unwrap();
    let running =
        until(&world, "the session records the turn on the same accepted request", || {
            running(&world).filter(|r| r["started"].as_u64().unwrap_or(0) != 0)
        });
    assert_eq!(sent(&world, &running).len(), 1);
    assert_eq!(starts(&world).len(), 1);
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(
        doctor["host"]["watch"]["running"]["delivery"]["execution_evidence"]["kind"],
        "session_turn"
    );
    printed(&world, &running, &["unobservable provider reported"], Some(0));
    until(&world, "explicit report", || ended(&world, "ended"));
}

#[test]
fn a_report_before_execution_proof_keeps_research_within_its_daily_bound() {
    let world = World::new();
    let dir = operator(&world);
    world.kit_models("claude-code", &["unplaced"]);
    world.with_kit(|kit| kit["harnesses"][0]["models"][0]["good_at"] = json!("research pending"));
    world.with_orca(|orca| {
        orca["prompt_stages"] = json!(["input_accepted"]);
        orca["prompt_observation"] = json!("unsupported");
        orca["send_screen"] = json!(["idle prompt; execution unproven"]);
    });
    run(&world, &dir);
    until(&world, "unproven research delivery", || {
        state(&world)["unavailable"].as_str().map(str::to_string)
    });
    let watch = running(&world).unwrap();
    assert_eq!(watch["started"], 0);
    printed(&world, &watch, &["research pending; explicit recovery report"], Some(0));
    let last = until(&world, "research recovery report", || ended(&world, "ended"));
    assert!(last["started"].as_u64().unwrap() > 0, "the report proves the round ran");
    let before = state(&world)["until"].as_u64().unwrap();
    until(&world, "look after research reported", || {
        state(&world)["until"].as_u64().filter(|at| *at > before)
    });
    assert_eq!(starts(&world).len(), 1);
    assert_eq!(sent(&world, &watch).len(), 1);
    assert!(running(&world).is_none());
    assert_eq!(state(&world)["research_started"], last["started"]);
}

#[test]
fn a_pre_journal_watch_keeps_its_recorded_id_and_captures_public_identity_on_upgrade() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault");
    run(&world, &dir);
    let watch = until(&world, "watch", || running(&world));
    world.kill_host_runner("KILL");
    let association = world.cache().join("afkconvoy/watch/terminal.json");
    std::fs::remove_file(&association).unwrap();
    // The pre-upgrade state still names exactly the Terminal we own. A
    // title collision must not replace it with the operator's session.
    world.with_orca(|orca| orca["terminals"][0]["title"] = json!("afkconvoy watch"));
    run(&world, &dir);
    until(&world, "the migrated association", || {
        let bytes = std::fs::read(&association).ok()?;
        let record: Value = serde_json::from_slice(&bytes).ok()?;
        (record["status"] == "running").then_some(record)
    });
    assert_eq!(running(&world).unwrap()["terminal"], watch["terminal"]);
    assert_eq!(starts(&world).len(), 1);
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["watch"]["association"]["terminal"], watch["terminal"]);
    assert!(doctor["host"]["watch"]["association"]["identity"]["ptyId"].is_string());
}

#[test]
fn a_round_records_only_usage_between_delivery_and_report_and_unknown_without_usage() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "a fault to account");
    run(&world, &dir);
    let watch = until(&world, "watch with fixture usage", || running(&world));
    assert_eq!(watch["harness"], "claude-code");
    let start = watch["started"].as_u64().unwrap();
    let brief = world
        .cache()
        .join("afkconvoy/watch")
        .join(watch["id"].as_str().unwrap())
        .join("brief-1.md");
    let pointer = afkconvoy_core::prompt::brief_pointer(&brief.display().to_string());
    let harness = afkconvoy_core::harness::Harness::ClaudeCode;
    let session = &harness.session_files(&dir.display().to_string(), true, start)[0];
    let root = world.home().join(harness.session_place(&dir.display().to_string()).under_home);
    let file = root.join(&session.path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut text = session.body.replace("\"brief\"", &serde_json::to_string(&pointer).unwrap());
    for (id, at, input) in
        [("before", start - 10, 100), ("round", start, 7), ("after", start + 100, 20)]
    {
        text.push_str(&format!(
            "{}\n",
            json!({"timestamp": at, "type": "assistant", "cwd": dir,
                "isSidechain": false, "message": {"id": id, "role": "assistant", "model": "fixture",
                "usage": {"input_tokens": input, "cache_read_input_tokens": 2,
                "cache_creation_input_tokens": 1, "output_tokens": 3}}})
        ));
    }
    std::fs::write(&file, text).unwrap();
    printed(&world, &watch, &["accounted"], Some(0));
    until(&world, "usage recorded", || ended(&world, "ended"));
    let usage = |id: &str| {
        let path = world.cache().join("afkconvoy/watch").join(id).join("usage.json");
        serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap()
    };
    let first = usage(watch["id"].as_str().unwrap());
    assert_eq!(first["start"], start);
    assert_eq!(first["end"], first["start"].as_u64().unwrap() + first["seconds"].as_u64().unwrap());
    assert_eq!(
        first["tokens"]["fixture"],
        json!({"input": 7, "cache_read": 2, "cache_write": 1, "output": 3, "reasoning": null})
    );

    paused(&world, "dsh", "a round with no usage");
    let second = until(&world, "second round", || {
        running(&world).filter(|round| round["id"] != watch["id"])
    });
    printed(&world, &second, &["no usage recorded"], Some(0));
    until(&world, "second usage recorded", || {
        ended(&world, "ended").filter(|round| round["id"] == second["id"])
    });
    let second = usage(second["id"].as_str().unwrap());
    assert!(second["tokens"].is_null());
}

#[test]
fn a_week_s_daybook_lessons_start_one_distillation_round_that_covers_its_span() {
    let world = World::new();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let yesterday = afkconvoy::daybook::day_at(now - 24 * 60 * 60);
    let lesson = "https://gh/discussions/1#c-1";
    world.with_gh(|gh| {
        gh["discussion_categories"] = json!(["Daybook"]);
        gh["discussions"] = json!([{"id": "discussion-1", "number": 1, "title": yesterday,
            "category": "Daybook", "url": "https://gh/discussions/1",
            "comments": [{"url": lesson, "body": "fixed the gate"}],
            "body": format!("Intro.\n\n<!--convoy:lessons-->\n## Lessons\n\n- [entry]({lesson}): the gate needs jq\n")}]);
    });
    world.name_origin(&world.clone, "acme/dev");
    world.list_checkout(&world.clone);
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "the distillation", || running(&world));
    let items = watch["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{watch}");
    let id = items[0]["id"].as_str().unwrap();
    assert!(id.starts_with("daybook-distill:") && id.ends_with(&format!(":{yesterday}")), "{id}");
    assert!(items[0]["reason"].as_str().unwrap().contains("1 lessons"), "{watch}");
    let told = brief(&world, &starts(&world)[0]);
    for instruction in [
        "Distill the daybook's lessons into their lasting homes",
        "a **draft** PR on dotfiles",
        "Never merge it",
        "afkconvoy daybook --distilled <file>",
        "no memory, precedent or proposal list",
    ] {
        assert!(told.contains(instruction), "{instruction}\n{told}");
    }
    printed(&world, &watch, &["carried the gate lesson to a ticket"], Some(0));
    until(&world, "the distillation ended", || ended(&world, "ended"));
    assert_eq!(state(&world)["distilling"]["until"], yesterday.as_str());
    let first = state(&world)["until"].as_u64().unwrap();
    until(&world, "a later look", || state(&world)["until"].as_u64().filter(|at| *at > first));
    assert_eq!(starts(&world).len(), 1, "a distillation waits out its week");
}

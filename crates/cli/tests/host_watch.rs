//! The host runner watches its host: no model while a look finds nothing to
//! act on, and one watch in the operator's directory when one does — kept
//! across a restart of the host runner, continued on another harness where
//! its own gives out, and ended at its hour. The stand-in orca runs no
//! command: a test writes what a watch's session printed to its transcript.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use afkconvoy_core::host_watch::EXITED;
use serde_json::{Value, json};

use crate::common::{World, asked};

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
fn brief(line: &str) -> String {
    let at =
        line.find("Your task brief is the file ").unwrap() + "Your task brief is the file ".len();
    let file = &line[at..line[at..].find(" — ").unwrap() + at];
    std::fs::read_to_string(file).unwrap()
}

/// Has the session of `watch` print `lines`, and its harness exit with
/// `status` where it says one: the shell then exits, and its terminal closes.
fn printed(world: &World, watch: &Value, lines: &[&str], status: Option<i32>) {
    let mut text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if let Some(status) = status {
        text.push_str(&format!("{EXITED} {status}\n"));
    }
    std::fs::write(watch["transcript"].as_str().unwrap(), text).unwrap();
    if status.is_some() {
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                if terminal["handle"] == watch["terminal"] {
                    terminal["open"] = json!(false);
                }
            }
        });
    }
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
        "list each in your printed record",
    ] {
        assert!(rules.contains(required), "stock instruction lacks {required}: {rules}");
    }
    let told = brief(&starts(&world)[0]);
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
    assert!(!brief(&starts(&world)[0]).contains("# The stock watch"));
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
    let told = brief(&starts(&world)[0]);
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
    assert_eq!(daily["model"], "fast", "the next watch uses the placed model");
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
    // A watch is independent of the issue worker's trust gap: its launch is
    // non-interactive and already trusted. The fake's gap affects dispatch.
    run(&world, &dir);
    let watch = until(&world, "the watch of the fleet fault", || running(&world));
    let told = brief(&starts(&world)[0]);
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
    assert!(line.contains(" --print "), "non-interactive: {line}");
    assert!(line.contains("--dangerously-skip-permissions"), "trusted: {line}");
    assert!(line.contains(&format!("tee -a '{}'", watch["transcript"].as_str().unwrap())));
    let told = brief(&started[0]);
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
    assert!(items[0].contains("`dsh`"), "{items:?}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["running"]["id"], next["id"]);
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
    assert!(starts(&world)[0].contains(" exec "), "{:?}", starts(&world));
    assert!(starts(&world)[0].contains(" --search "), "the watch can search the web");

    let out = "Usage limit reached; increase your limits to continue using Codex";
    printed(&world, &first, &["checked kimi", out], None);
    let next = until(&world, "the watch on Claude Code", || {
        running(&world).filter(|running| running["harness"] == "claude-code")
    });
    assert_eq!((&next["id"], &next["started"]), (&first["id"], &first["started"]));
    assert_eq!(next["attempts"][0]["harness"], "codex");
    assert_eq!(next["attempts"][0]["did"], json!(["checked kimi", out]));
    let started = starts(&world);
    assert_eq!(started.len(), 2, "{started:?}");
    let told = brief(&started[1]);
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
    assert_eq!(last["did"], json!(["checked kimi", out, "API Error: 500", "fixed it"]));
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
    assert_eq!(open(&world, &dir), [OPERATORS], "its own terminal alone is ended");

    paused(&world, "claude-code", "out of tokens");
    let why = until(&world, "no harness to start one", || {
        state(&world)["unavailable"].as_str().map(String::from)
    });
    assert!(why.contains("no harness free of a fault"), "{why}");
    assert_eq!(starts(&world).len(), 1);
    let doctor = world.afk(&["doctor", "--all"]);
    assert_eq!(doctor.code, 1, "{}", doctor.json);
    let findings = doctor.json["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f["what"] == "no harness can start this host's watch"));
    assert_eq!(doctor.json["host"]["watch"]["unavailable"], json!(why));
}

/// Fake process readings never count or signal processes of the test host.
fn resource_host(world: &World) {
    use std::os::unix::fs::PermissionsExt;
    let ps = world.root.join("bin/ps");
    std::fs::write(&ps, "#!/bin/sh\ncat \"${0%/*}/../host-processes\"\n").unwrap();
    std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        world.root.join("host-processes"),
        "999990 2 10485760 0:00.00 outside-memory-holder\n",
    )
    .unwrap();
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
        "999990 2 10485760 0:00.00 outside-memory-holder\n999980 2 {} 0:00.00 worker TMPDIR={}\n",
        mib * 1024, tmp.display())).unwrap();
    path
}

#[test]
fn a_hot_worker_starts_the_host_watch_with_its_reading_and_evidence() {
    let world = World::new();
    resource_host(&world);
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
    let told = brief(&starts(&world)[0]);
    for text in ["hot worker", "#7", "resident", path.to_str().unwrap(), "evidence.md", "terminal"]
    {
        assert!(told.contains(text), "missing {text}: {told}");
    }
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
        state["admission"][world.clone.to_str().unwrap()] =
            json!({"holding": holding, "stages": []});
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
    let told = brief(&starts(&world)[0]);
    for text in ["admission has held back", "load", "600", "reading", "cores", "evidence.md"] {
        assert!(told.contains(text), "missing {text}: {told}");
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

#[cfg(target_os = "macos")]
#[test]
fn pressure_starts_one_watch_across_fleets_and_no_worker_turns_hot() {
    let world = World::new();
    resource_host(&world);
    resource_worker(&world, 7, 620);
    std::fs::write(world.root.join("host-pressure"), "2").unwrap();
    held_since(&world, true);
    let dir = operator(&world);
    run(&world, &dir);
    let watch = until(&world, "pressure's watch", || running(&world));
    assert!(watch["items"].to_string().contains("resource:pressure:"), "{watch}");
    let told = brief(&starts(&world)[0]);
    assert!(told.contains("under memory pressure"), "{told}");
    assert!(told.contains("level"), "{told}");
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
    printed(&world, &first, &["checked pi"], Some(0));
    let next = until(&world, "watch on the new resource episode", || {
        running(&world).filter(|w| w["id"] != first["id"])
    });
    assert!(next["items"].to_string().contains("resource:hot:"), "{next}");
    assert_eq!(starts(&world).len(), 2);
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

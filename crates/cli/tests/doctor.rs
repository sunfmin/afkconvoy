//! `afkconvoy doctor`: the state of the fleet on this host and what is wrong
//! with it, read with nothing written; and the log `run` keeps for it. What
//! each finding is made from is `doctor.rs`'s own tests'; these are the reads.

use crate::common;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Answer, World, commit, git, write};
use serde_json::{Value, json};

fn doctor(world: &World, now: &str) -> Answer {
    world.afk(&["doctor", "--host", "h1", "--now", now])
}

fn run(world: &World, extra: &[&str]) -> Vec<String> {
    let mut args = vec!["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    args.extend(extra);
    world.afk_lines(&args).0
}

/// The lines this world's fleet kept. The host's independent watch events
/// are checked by `host_watch`.
fn log(world: &World) -> Vec<Value> {
    let runs = world.cache().join("afkconvoy/runs");
    let files = std::fs::read_dir(&runs).unwrap().map(|entry| entry.unwrap().path());
    // Beside it, the lock its writers take.
    let files: Vec<PathBuf> =
        files.filter(|file| file.extension().is_some_and(|ext| ext == "jsonl")).collect();
    assert_eq!(files.len(), 1, "one log for the one repo: {files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|entry| entry["instance"] != afkconvoy::host_watch::INSTANCE)
        .collect()
}

/// The lines of `log` that are ticks' ends.
fn ticks(log: &[Value]) -> Vec<&Value> {
    log.iter().filter(|line| !line["tick"].is_null()).collect()
}

/// The findings of `answer` whose `what` contains `what`.
fn found<'a>(answer: &'a Value, what: &str) -> Vec<&'a Value> {
    let findings = answer["findings"].as_array().unwrap().iter();
    findings.filter(|finding| finding["what"].as_str().unwrap().contains(what)).collect()
}

fn named_grok() -> World {
    let world = World::new();
    world.hold_every_harness();
    world.with_kit(|kit| {
        let entries = kit["harnesses"].as_array_mut().unwrap();
        let at = entries.iter().position(|held| held["harness"] == "grok").unwrap();
        let grok = entries.remove(at);
        entries.insert(0, grok);
    });
    world
}

fn grok_login_questions(world: &World) -> usize {
    world.shell_asked().iter().filter(|question| *question == "grok models").count()
}

#[test]
fn a_harness_login_check_asks_once_more_only_after_a_logged_out_answer() {
    let world = named_grok();
    world.login_answers(
        "grok",
        "You are not authenticated.",
        "You are logged in with grok.com.\nAvailable models:\n  * grok-4.7 (default)\n",
    );
    let answer = doctor(&world, "1000").json;
    assert!(found(&answer, "`grok` is not logged in").is_empty(), "{answer}");
    assert_eq!(answer["host"]["tools"][3]["state"], "ready");
    assert_eq!(grok_login_questions(&world), 2);

    let ready = named_grok();
    doctor(&ready, "1000");
    assert_eq!(grok_login_questions(&ready), 1, "a ready first answer is not asked again");
}

#[test]
fn a_logged_out_finding_keeps_the_second_answer_without_its_credentials() {
    let world = named_grok();
    world.login_answers(
        "grok",
        "the first answer expired",
        "You are not authenticated.\ntoken=second-secret api_key: \"another-secret\"\n",
    );
    let answer = doctor(&world, "1000");
    assert_eq!(answer.code, 1);
    let finding = found(&answer.json, "`grok` is not logged in")[0];
    let evidence = finding["evidence"].as_str().unwrap();
    assert!(evidence.contains("exit status: 0"), "{evidence}");
    assert!(evidence.contains("You are not authenticated."), "{evidence}");
    assert!(evidence.contains("token=[redacted]"), "{evidence}");
    assert!(evidence.contains("api_key: \"[redacted]\""), "{evidence}");
    assert!(!evidence.contains("second-secret") && !evidence.contains("another-secret"));
    assert!(!evidence.contains("the first answer expired"), "{evidence}");
    assert_eq!(grok_login_questions(&world), 2);
}

/// The checkout, as a fix line quotes a path it inserts.
fn quoted(world: &World) -> String {
    format!("'{}'", world.clone.display())
}

/// The arguments of a fix line, quotes taken off: the line is for a shell.
fn shell_args(line: &str) -> Vec<String> {
    line.split_whitespace()
        .skip(1)
        .map(|word| {
            word.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
                .unwrap_or(word)
                .to_string()
        })
        .collect()
}

#[test]
fn a_blocked_claim_whose_blockers_closed_has_a_working_doctor_fix() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    world.issue(41, &["ready-for-agent"]);
    world.comment(7, "<!--convoy:verdict phase=blocked blocked_by=41-->\nwaiting");
    world.close_issue(41);
    world.with_orca(|orca| {
        let terminal = &mut orca["terminals"][0];
        terminal["agent"] = json!({"state": "idle", "stateStartedAt": 1000000});
        terminal["lastOutputAt"] = json!(1000000);
    });

    let answer = doctor(&world, "3000").json;
    let stopped = found(&answer, "the worker of #7 has been stopped");
    assert_eq!(stopped.len(), 1, "{answer}");
    let fix = stopped[0]["fix"].as_str().unwrap();
    assert_eq!(fix, format!("afkconvoy dispatch -C {} --issue 7 --instance alpha", quoted(&world)));
    assert!(!stopped[0]["what"].as_str().unwrap().contains("declared nothing"));
    let mut words = shell_args(fix);
    words.extend(["--now".into(), "3000".into()]);
    let args: Vec<&str> = words.iter().map(String::as_str).collect();
    let continued = world.afk(&args).ok();
    assert_eq!(continued["started"], true, "{continued}");
    assert_eq!(world.open_terminals().len(), 1);
    assert_eq!(world.orca()["terminals"].as_array().unwrap().len(), 2);
    let again = world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "3090"]).ok();
    assert_eq!(again["did"][0]["outcome"], "nothing", "{again}");
}

#[test]
fn a_worker_left_alone_by_a_fleet_lock_waits_until_its_nudge() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", &now.to_string()]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    world.with_orca(|orca| {
        orca["terminals"][0]["agent"] = json!({"state": "done", "stateStartedAt": now * 1000});
        orca["terminals"][0]["lastOutputAt"] = json!(now * 1000);
    });
    world
        .afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &(now + 1000).to_string()])
        .ok();
    let locked = doctor(&world, &(now + 1000).to_string()).json;
    let claim = locked["claims"].as_array().unwrap().iter().find(|c| c["issue"] == 3).unwrap();
    assert_eq!(claim["worker"], json!({"state": "waiting_on_lock", "locks": [7]}));
    assert!(found(&locked, "the worker of #3 has been stopped").is_empty(), "{locked}");

    world.close_issue(7);
    let nudge = world
        .afk(&[
            "nudge",
            "--issue",
            "3",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            &(now + 1100).to_string(),
        ])
        .ok();
    assert_eq!(nudge["action"], "nudged");
    let before = doctor(&world, &(now + 1399).to_string()).json;
    assert!(found(&before, "the worker of #3 has been stopped").is_empty(), "{before}");
    let after = doctor(&world, &(now + 1401).to_string()).json;
    let stopped = found(&after, "the worker of #3 has been stopped");
    assert_eq!(stopped.len(), 1, "{after}");
    assert!(stopped[0]["evidence"].as_str().unwrap().contains("after its nudge"));
}

#[test]
fn doctor_routes_other_declared_verdicts() {
    for (phase, command) in [
        ("blocked blocked_by=41", "park --issue 7"),
        ("already-satisfied", "tick"),
        ("escalate", "tick"),
    ] {
        let world = World::new();
        world.issue(7, &["ready-for-agent"]);
        run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
        world.issue(41, &["ready-for-agent"]);
        world.comment(7, &format!("<!--convoy:verdict phase={phase}-->\nwhy"));
        world.with_orca(|orca| {
            let terminal = &mut orca["terminals"][0];
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": 1000000});
            terminal["lastOutputAt"] = json!(1000000);
        });
        let answer = doctor(&world, "3000").json;
        let stopped = found(&answer, "the worker of #7 has been stopped");
        assert_eq!(stopped.len(), 1, "{answer}");
        let fix = stopped[0]["fix"].as_str().unwrap();
        let expected = if command.starts_with("park") {
            format!("afkconvoy park -C {} --issue 7 --instance alpha", quoted(&world))
        } else {
            format!("afkconvoy tick {} --instance alpha", quoted(&world))
        };
        assert_eq!(fix, expected);
        let mut words = shell_args(fix);
        words.extend(["--now".into(), "3000".into()]);
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        let acted = world.afk(&args).ok();
        assert!(acted["errors"].is_null() || acted["errors"] == json!([]), "{acted}");
    }
}

#[test]
fn a_config_with_no_gate_seconds_is_a_finding_and_one_that_sets_it_is_none() {
    let world = World::new();
    world.set_gate_config("gate_command: ./gate.sh");
    let answer = doctor(&world, "1100").json;
    let undeclared = found(&answer, "declares no gate limit");
    assert_eq!(undeclared.len(), 1, "{answer}");
    assert_eq!(undeclared[0]["severity"], "info");
    assert_eq!(undeclared[0]["fix"], "docs/agents/afkconvoy.md: gate_seconds");
    assert_eq!(undeclared[0]["see"], "afkconvoy help troubleshooting");

    world.set_gate_config("gate_command: ./gate.sh\ngate_seconds: 900");
    let answer = doctor(&world, "1100").json;
    assert_eq!(found(&answer, "declares no gate limit"), [] as [&Value; 0], "{answer}");
}

#[test]
fn a_retired_concurrency_is_info_wherever_it_is_named() {
    for (named, kept) in [(Some(4), None), (None, Some(5)), (Some(4), Some(5))] {
        let world = World::new();
        let more = named.map(|named| format!("\nconcurrency: {named}")).unwrap_or_default();
        world.set_gate_config(&format!("gate_command: ./gate.sh\ngate_seconds: 600{more}"));
        if let Some(kept) = kept {
            world.say_concurrency(&world.clone, kept);
        }
        let answer = doctor(&world, "1100").json;
        let findings = found(&answer, "which is ignored");
        let mut fixes: Vec<String> = vec![];
        if kept.is_some() {
            fixes.push(format!("afkconvoy run {} --concurrency default", world.clone.display()));
        }
        if named.is_some() {
            fixes.push("docs/agents/afkconvoy.md: concurrency".to_owned());
        }
        let said: Vec<&str> = findings.iter().map(|f| f["fix"].as_str().unwrap()).collect();
        assert_eq!(said, fixes, "config names {named:?}, checkout keeps {kept:?}: {answer}");
        for finding in findings {
            assert_eq!(finding["severity"], "info", "{finding}");
            let what = finding["what"].as_str().unwrap();
            assert!(what.contains("admission alone decides each start"), "{what}");
            assert_eq!(finding["see"], "afkconvoy help troubleshooting");
        }
    }
}

#[test]
fn a_fleet_with_no_fault_answers_its_keys_and_no_finding_and_writes_nothing() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    world.issue(5, &["bug"]);
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    world.watch_pushes(None);
    world.gh_calls();
    let refs = world.remote(&["for-each-ref"]);
    // the worker of #3 is at work, and so is the `run`: nothing is wrong but
    // that no `run` is alive, which this world cannot have — see below
    let answer = doctor(&world, "1100").json;
    let keys: Vec<&str> = answer.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "candidates",
            "claims",
            "codex_workers",
            "deliveries",
            "findings",
            "fleet",
            "host",
            "issues",
            "refusals",
            "targets",
            "waiting_on_human"
        ]
    );
    assert_eq!(answer["candidates"], json!([]));
    // both open issues are destinations on the configured base; only the dispatched one is pinned
    let targets = answer["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2, "{answer}");
    assert!(
        targets.iter().all(|row| row["delivery"].is_null()
            && row["target_branch"] == json!("main")
            && row.get("error").is_none()),
        "{targets:?}"
    );
    let pinned: Vec<u64> = targets
        .iter()
        .filter(|row| row["pinned"] == json!(true))
        .map(|row| row["issue"].as_u64().unwrap())
        .collect();
    assert_eq!(pinned, [3], "{targets:?}");
    assert!(answer.get("proposals").is_none());
    assert_eq!(answer["refusals"], json!({"3": [], "5": []}));
    assert_eq!(answer["waiting_on_human"], json!([]));

    // nothing on the remote, probes included, and nothing on GitHub
    assert_eq!(world.probes(), [] as [String; 0]);
    assert_eq!(world.pushes(), [] as [String; 0]);
    assert_eq!(world.remote(&["for-each-ref"]), refs);
    for call in world.gh_calls() {
        let writes = ["POST", "PATCH", "PUT", "DELETE", "edit", "create", "close", "comment"];
        assert!(!call.iter().any(|arg| writes.contains(&arg.as_str())), "doctor wrote: {call:?}");
    }

    let host = &answer["host"];
    let tools: Vec<(&str, &str)> = host["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| (tool["name"].as_str().unwrap(), tool["state"].as_str().unwrap()))
        .collect();
    assert_eq!(tools, [("git", "ready"), ("gh", "ready"), ("orca", "ready")]);
    // the command the `run` here said it started with
    assert_eq!(host["worker_commands"][0]["command"], "agent");
    assert_eq!(host["worker_commands"][0]["from"], "host_kit");

    // the instance is found with no `--instance`: its log names it
    let fleet = &answer["fleet"];
    assert_eq!(fleet["instance"], "alpha");
    assert_eq!(fleet["alive"], false);
    assert_eq!(fleet["heartbeat_age"], 100);
    assert_eq!(fleet["heartbeats"], json!({"found": 1, "live": ["alpha"]}));
    assert_eq!(fleet["last_tick"]["count"], 1);
    assert_eq!(fleet["last_tick"]["line"], "dispatched #3 · holding 1 · frontier 0");
    assert_eq!(fleet["last_tick"]["did"][0]["do"], "dispatch");
    assert_eq!(fleet["failed_steps"], json!([]));

    let claim = &answer["claims"][0];
    assert_eq!(claim["issue"], 3);
    assert_eq!(claim["status"], "no_pr");
    assert_eq!(claim["held_for"], 100);
    assert!(claim["worktree"].as_str().unwrap().contains("issue-3"), "{claim}");
    assert_eq!(answer["claims"].as_array().unwrap().len(), 1);

    let issues = &answer["issues"];
    assert_eq!(issues["frontier"], json!([]));
    assert_eq!(issues["excluded"][1]["reason"], "no ready-for-agent label");
    assert_eq!(issues["free_slots"], 1);

    // the one finding is the one this world has: its `run` ended
    assert_eq!(answer["findings"].as_array().unwrap().len(), 1, "{answer}");
    assert_eq!(
        found(&answer, "no `afkconvoy run` is alive")[0]["fix"],
        format!("afkconvoy run {}", quoted(&world))
    );
}

#[test]
fn another_accounts_host_runner_on_this_machine_is_named_from_the_process_table() {
    let world = World::new();
    write(
        &world.root,
        "other-accounts",
        "  70001 felix   4100   1.5  20480 /opt/homebrew/bin/afkconvoy host-runner\n\
         \x20 70001 felix   4200 148.5 1048576 cargo build\n",
    );
    let answer = doctor(&world, "1000").ok();
    assert_eq!(
        answer["host"]["other_accounts"],
        json!([{"account": "felix", "uid": 70001, "pid": 4100,
            "program": "/opt/homebrew/bin/afkconvoy", "cores": 1.5,
            "memory_bytes": (20480 + 1048576) * 1024, "processes": 2}]),
        "{answer}"
    );
    // This world's account runs no host runner: the fleet may be felix's.
    assert_eq!(answer["host"]["exclusive"]["status"], "no_runner", "{answer}");
    let said = found(&answer, "works on this machine under another account");
    assert_eq!(said.len(), 1, "{answer}");
    assert_eq!(said[0]["severity"], "info");
    assert!(said[0]["what"].as_str().unwrap().contains("felix (pid 4100)"), "{answer}");
    assert_eq!(said[0]["fix"], "su - felix -c 'afkconvoy doctor --all'");
}

#[test]
fn an_afkconvoy_first_on_path_of_another_release_is_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let first = world.root.join("bin/afkconvoy");
    std::fs::remove_file(&first).unwrap();
    std::fs::write(&first, "#!/bin/sh\necho 'afkconvoy 0.0.1'\n").unwrap();
    std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o755)).unwrap();
    let answer = doctor(&world, "1000").ok();
    let said = found(&answer, "is first on PATH");
    assert_eq!(said.len(), 1, "{answer}");
    assert_eq!(said[0]["severity"], "warning");
    let what = said[0]["what"].as_str().unwrap();
    assert!(what.starts_with(&format!("afkconvoy 0.0.1, at {}", first.display())), "{what}");
    assert!(what.contains(&format!("would start afkconvoy {}", afkconvoy::VERSION)), "{what}");
    assert_eq!(said[0]["fix"], "which -a afkconvoy");
}

#[test]
fn a_repo_no_fleet_ever_ran_in_has_no_finding_no_log_and_exits_zero() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let answer = doctor(&world, "1000").ok();
    assert_eq!(answer["findings"], json!([]));
    assert_eq!(answer["fleet"]["instance"], Value::Null);
    assert_eq!(answer["fleet"]["log"], "no log");
    assert_eq!(answer["fleet"]["last_tick"], Value::Null);
    assert_eq!(answer["claims"], json!([]));
    assert_eq!(answer["issues"]["frontier"][0]["number"], 3);
    // with no `run` to say, the command is the one a `run` would use now
    assert_eq!(answer["host"]["worker_commands"][0]["from"], "host_kit");
    assert_eq!(answer["host"]["exclusive"]["train"], Value::Null);
    // An unbound repo calls for no harness by name: its one command is the
    // host kit's first's, which holds no model here — and neither is found.
    let commands = answer["host"]["worker_commands"].as_array().unwrap();
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(
        (&commands[0]["harness"], &commands[0]["called_for_by"]),
        (&json!(null), &json!([]))
    );
    assert_eq!(commands[0]["command"], "agent");
    let kit = &answer["host"]["host_kit"];
    assert_eq!(kit["harnesses"][0]["harness"], "claude-code");
    assert_eq!(kit["harnesses"][0]["models"], json!([]));
}

#[test]
fn the_host_s_exclusive_queue_is_shown_what_runs_first_then_what_waits() {
    let world = World::new();
    let _host_runner = world.host_held();
    let ran = world.root.join("ran");
    let exclusive = |now: &str, issue: Option<&str>, script: &str| {
        let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
        command.env_remove("AFKCONVOY_ISSUE");
        if let Some(issue) = issue {
            command.env("AFKCONVOY_ISSUE", issue);
        }
        let args = ["--now", now, "exclusive", "--", "sh", "-c", script];
        common::FixtureProcess::spawn(
            command.args(args).stdout(Stdio::null()).stderr(Stdio::null()),
        )
    };
    let path = ran.display().to_string();
    let shell = format!("'{}'", path.replace('\'', r"'\''"));
    let touched = format!("touch {shell}; sleep 30");
    let running = exclusive("900", Some("12"), &touched);
    while !ran.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let waiting = exclusive("950", None, "true");
    let shown = || doctor(&world, "1000").ok()["host"]["exclusive"].clone();
    while shown()["waiting"].as_array().is_none_or(Vec::is_empty) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let exclusive = shown();
    let dir = std::fs::canonicalize(&world.clone).unwrap().display().to_string();
    assert_eq!(exclusive["status"], "available", "{exclusive}");
    assert_eq!(exclusive["observed_at"], 1000, "{exclusive}");
    assert_eq!(exclusive["worker"], json!({"capacity": 1, "used": 1, "free": 0}), "{exclusive}");
    let [first] = exclusive["running"].as_array().unwrap().as_slice() else {
        panic!("one runs: {exclusive}");
    };
    // A path with a space or a quote is quoted again, so the one line is still that argument.
    let shown = if path.contains([' ', '\'']) { shell } else { path };
    assert_eq!(first["command"], format!("touch {shown}; sleep 30"));
    assert_eq!((&first["dir"], &first["issue"]), (&json!(dir), &json!(12)));
    assert_eq!((&first["pid"], &first["asked"]), (&json!(running.id()), &json!(900)));
    assert_eq!(
        (&first["lane"], &first["scheduling"]["state"]),
        (&json!("worker"), &json!("running"))
    );
    assert!(first["ran"].is_u64(), "{exclusive}");
    let [next] = exclusive["waiting"].as_array().unwrap().as_slice() else {
        panic!("one waits: {exclusive}");
    };
    assert_eq!((&next["command"], &next["dir"]), (&json!("true"), &json!(dir)));
    assert_eq!((&next["issue"], &next["ran"]), (&Value::Null, &Value::Null));
    assert_eq!((&next["pid"], &next["asked"]), (&json!(waiting.id()), &json!(950)));
    assert_eq!(next["waiting_seconds"], 50);
    // Not started is waiting in the observation, with no turn's result word.
    assert!(next.get("waiting").is_none(), "{exclusive}");
    assert_eq!(next["scheduling"]["state"], "waiting");
    let kinds: Vec<_> =
        next["scheduling"]["reasons"].as_array().unwrap().iter().map(|r| &r["kind"]).collect();
    assert_eq!(kinds, [&json!("same_repo"), &json!("worker_capacity")], "{exclusive}");
    common::fixture_lease_observed();
    drop(running);
    drop(waiting);
}

#[test]
fn a_critical_finding_exits_non_zero_with_the_same_answer() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    let unwell = doctor(&world, "1100");
    assert_eq!(unwell.code, 1);
    assert_eq!(unwell.json["error"], Value::Null, "the answer, not an error: {}", unwell.json);
    let finding = &unwell.json["findings"][0];
    assert_eq!(finding["severity"], "critical");
    assert!(finding["what"].as_str().unwrap().contains("alpha holds #3"), "{finding}");
    assert!(finding["evidence"].as_str().unwrap().contains("its heartbeat is 1m old"));
    assert_eq!(finding["fix"], format!("afkconvoy run {}", quoted(&world)));
    assert_eq!(finding["see"], "afkconvoy help troubleshooting");
}

#[test]
fn the_instance_is_found_from_the_claims_stamped_with_this_host_where_there_is_no_log() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(5, &["ready-for-agent"]);
    let claim = |issue: &str, instance: &str, host: &str, now: &str| {
        let args =
            ["claim", "--issue", issue, "--instance", instance, "--host", host, "--now", now];
        world.afk(&args).ok();
    };
    claim("3", "old", "h1", "900");
    claim("5", "beta", "h2", "1000");
    let answer = doctor(&world, "1100").json;
    assert_eq!(answer["fleet"]["instance"], "old");
    assert_eq!(answer["fleet"]["log"], "no log");
    let held: Vec<u64> =
        answer["claims"].as_array().unwrap().iter().map(|c| c["issue"].as_u64().unwrap()).collect();
    assert_eq!(held, [3], "another host's claim is not this fleet's");
}

#[test]
fn run_logs_a_line_as_it_starts_one_as_each_tick_starts_and_ends_and_prints_each() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    world.with_orca(|orca| orca["create_fails"] = json!([3]));
    let args = ["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    let more = ["--instance", "alpha", "--ticks", "2", "--now", "1000"];
    let (lines, _) = world.afk_every_line(&[&args[..], &more[..]].concat());
    let log = log(&world);
    assert_eq!(log.len(), 5, "{log:?}");
    assert_eq!(log[0]["start"]["worker_command"], "agent");
    assert_eq!(log[1]["tick_started"], json!({"count": 1, "cause": "start"}));
    assert_eq!(log[3]["tick_started"], json!({"count": 2, "cause": "timer"}));
    for line in &log {
        assert_eq!(
            (&line["instance"], &line["host"], &line["at"]),
            (&json!("alpha"), &json!("h1"), &json!(1000))
        );
    }
    // stdout holds every line of the log, in its order
    let printed: Vec<&String> = lines.iter().filter(|line| !line.starts_with("watch: ")).collect();
    assert_eq!(printed.len(), log.len(), "{lines:?}");
    assert_eq!(printed[0], "start: run as alpha on h1, its workers started with `agent`");
    assert_eq!(printed[1], "tick 1: started (start)");
    assert_eq!(printed[3], "tick 2: started (timer)");
    for (count, at) in [(1, 2), (2, 4)] {
        let tick = &log[at]["tick"];
        assert_eq!(tick["count"], count);
        // what `run` printed, and what `tick` answers
        let line = tick["line"].as_str().unwrap();
        assert_eq!(*printed[at], format!("tick {count}: {line} · next in {}s", tick["sleep"]));
        for key in ["did", "errors", "held", "frontier_remaining", "locks"] {
            assert!(!tick["answer"][key].is_null(), "the tick's answer lost {key}: {tick}");
        }
    }
    let failed = &log[2]["tick"]["answer"]["errors"][0];
    assert_eq!((&failed["do"], &failed["issue"]), (&json!("dispatch"), &json!(3)));

    // a second `run` writes on: nothing a `run` wrote is lost when it ends
    run(&world, &["--instance", "beta", "--ticks", "1", "--now", "2000"]);
    let more = self::log(&world);
    assert_eq!(more[..5], log[..]);
    assert_eq!(more.len(), 8);
    assert_eq!(more[5]["instance"], "beta");

    // …and `doctor` lists the starts of both, newest first, with their causes
    let answer = doctor(&world, "2000").json;
    let events = answer["fleet"]["events"].as_array().unwrap();
    let said: Vec<(&Value, &Value, &Value)> = events
        .iter()
        .filter(|event| event["instance"] != afkconvoy::host_watch::INSTANCE)
        .map(|event| (&event["at"], &event["event"], &event["instance"]))
        .collect();
    assert_eq!(
        said,
        [
            (&json!(2000), &json!("start"), &json!("beta")),
            (&json!(1000), &json!("start"), &json!("alpha"))
        ]
    );
    assert_eq!(events[0]["cause"], "afkconvoy run started, its workers with `agent`");
}

#[test]
fn a_tick_that_failed_whole_is_logged_and_three_in_a_row_are_a_finding() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.with_gh(|gh| gh["fail"] = json!(["pr list"]));
    run(&world, &["--instance", "alpha", "--ticks", "3", "--now", "1000"]);
    let log = log(&world);
    let last = ticks(&log)[2];
    assert!(last["tick"]["error"].as_str().unwrap().starts_with("gh pr list failed"), "{log:?}");
    assert_eq!(last["tick"]["answer"], Value::Null);

    world.with_gh(|gh| gh["fail"] = json!([]));
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let failed = found(&unwell.json, "`tick` failed in each of the last 3 ticks (1 to 3)");
    assert_eq!(failed[0]["severity"], "critical");
    assert_eq!(failed[0]["fix"], format!("afkconvoy plan {} --instance alpha", quoted(&world)));
    let steps = unwell.json["fleet"]["failed_steps"].as_array().unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!((&steps[0]["tick"], &steps[0]["do"]), (&json!(3), &json!("tick")));
}

#[test]
fn a_step_that_fails_tick_after_tick_is_critical() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world
        .afk(&["dispatch", "--issue", "3", "--instance", "alpha", "--host", "h1", "--now", "1000"])
        .ok();
    // An unreadable worker state keeps failing at every tick. Failed
    // startup is bounded instead: a second failed start ends its claim.
    world.with_orca(|orca| orca["ps_fails"] = json!(true));
    run(&world, &["--instance", "alpha", "--ticks", "4", "--now", "1000"]);
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let failed = found(&unwell.json, "`judge #3` failed in each of the last 3 ticks (2 to 4)");
    assert_eq!(failed[0]["severity"], "critical", "{}", unwell.json);
    assert_eq!(unwell.json["fleet"]["failed_steps"].as_array().unwrap().len(), 4);
}

#[test]
fn a_worker_that_fails_to_start_is_a_finding_after_one_tick() {
    for knob in ["create_fails", "never_ready"] {
        let world = World::new();
        world.issue(3, &["ready-for-agent"]);
        world.with_orca(|orca| {
            orca[knob] = if knob == "create_fails" { json!([3]) } else { json!(true) };
        });
        run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
        let answer = doctor(&world, "1000").json;
        let failures = found(&answer, "`dispatch #3` failed");
        assert_eq!(failures.len(), 1, "{knob}: {answer}");
        assert_eq!(
            failures[0]["fix"],
            format!("afkconvoy dispatch -C {} --issue 3 --instance alpha", quoted(&world))
        );
        assert_eq!(failures[0]["evidence"], answer["fleet"]["failed_steps"][0]["error"]);
    }
}

#[test]
fn a_brief_that_starts_no_turn_is_named_with_its_screen_in_doctor_and_on_the_issue() {
    for screen in
        [include_str!("screens/kimi-migration.txt"), include_str!("screens/kimi-empty-input.txt")]
    {
        let world = World::new();
        world.set_gate_config("gate_command: ./gate.sh\nescalate_to: human");
        world.issue(3, &["ready-for-agent"]);
        world.with_orca(|orca| {
            orca["prompt_stages"] = json!(["input_accepted"]);
            orca["send_screen"] = json!(screen.lines().collect::<Vec<_>>());
        });
        run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
        let answer = doctor(&world, "1000").json;
        let failures = found(&answer, "`dispatch #3` failed");
        assert_eq!(failures.len(), 1, "{answer}");
        let evidence = failures[0]["evidence"].as_str().unwrap();
        assert!(
            evidence.contains("started no turn") && evidence.contains(screen.trim()),
            "{evidence}"
        );
        run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1090"]);
        let comments = world.comments(3).join("\n");
        assert!(
            comments.contains("worker start failed again") && comments.contains(screen.trim()),
            "{comments}"
        );
        assert!(world.labels(3).contains(&"ready-for-human".into()));
    }
}

#[test]
fn an_orca_that_does_not_know_the_checkout_or_is_not_running_is_critical() {
    let world = World::new();
    world.with_orca(|orca| orca["repos"] = json!([]));
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let add = format!("orca repo add --path '{}'", world.clone.display());
    let unknown = found(&unwell.json, "`orca` does not know this checkout");
    assert_eq!(unknown[0]["fix"], add.as_str(), "{}", unwell.json);
    assert_eq!(unwell.json["host"]["tools"][2]["state"], "knows_no_checkout");
}

#[test]
fn state_that_cannot_be_read_is_a_finding_and_the_host_is_still_answered() {
    let world = World::new();
    world.with_gh(|gh| gh["fail"] = json!(["pr list"]));
    world.logged_out("gh");
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let answer = &unwell.json;
    assert_eq!(found(answer, "`gh` is not logged in")[0]["fix"], "gh auth login");
    let unread = found(answer, "the fleet's state cannot be read");
    assert!(unread[0]["evidence"].as_str().unwrap().contains("gh pr list failed"), "{answer}");
    assert_eq!(answer["issues"], Value::Null);
    assert_eq!(answer["claims"], json!([]));
    assert_eq!(answer["host"]["tools"][1], json!({"name": "gh", "state": "logged_out"}));
}

/// Starts a `run` that never ends, and kills it when dropped.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(world: &World) -> Running {
    let args =
        ["run", "--tracker", "default", "--foreground", "--instance", "alpha", "--host", "h1"];
    let child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let running = Running(child);
    // its first tick is over once its log says so — the log, not the lock
    // its writers take beside it, which may be there first
    let started = Instant::now();
    let runs = world.cache().join("afkconvoy/runs");
    let kept = || {
        let files = std::fs::read_dir(&runs).into_iter().flatten().flatten();
        files.into_iter().any(|file| file.path().extension().is_some_and(|ext| ext == "jsonl"))
    };
    while !kept() || ticks(&log(world)).is_empty() {
        assert!(started.elapsed() < Duration::from_secs(60), "the run never logged a tick");
        std::thread::sleep(Duration::from_millis(50));
    }
    running
}

#[test]
fn a_live_run_is_seen_alive_and_a_claim_whose_worker_is_gone_is_a_finding() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let running = start(&world);
    let at = ticks(&log(&world))[0]["at"].as_u64().unwrap() + 5;
    let now = at.to_string();
    world.with_orca(|orca| {
        let terminal = &mut orca["terminals"][0];
        terminal["agent"] = json!({"state": "working", "stateStartedAt": at * 1000});
        terminal["lastOutputAt"] = json!(at * 1000);
    });
    let answer = doctor(&world, &now).ok();
    assert_eq!(answer["fleet"]["alive"], true);
    assert_eq!(answer["fleet"]["instance"], "alpha");
    assert_eq!(answer["claims"][0]["worker"]["state"], "busy");
    assert_eq!(answer["findings"], json!([]), "a fleet at work has no finding");

    // Orca dropping its agent report does not end the turn the session still
    // has open, however long the screen has been quiet.
    world.with_orca(|orca| {
        orca["terminals"][0]["agent"] = json!(null);
        orca["terminals"][0]["lastOutputAt"] = json!((at - 400) * 1000);
    });
    let answer = doctor(&world, &now).ok();
    assert_eq!(answer["claims"][0]["worker"]["state"], "busy");

    // The session says that turn ended 400s ago.
    world.with_orca(|orca| {
        orca["terminals"][0]["agent"] =
            json!({"state": "idle", "stateStartedAt": (at - 400) * 1000});
    });
    let answer = doctor(&world, &now).ok();
    let worker = &answer["claims"][0]["worker"];
    assert_eq!(worker["state"], "stopped");
    assert_eq!(worker["quiet"], 400);
    assert_eq!(worker["harness"], "claude-code");
    assert_eq!(worker["recorded_harness"], "claude-code");
    assert_eq!(worker["harness_mismatch"], false);
    assert_eq!(worker["orca"]["state"], "idle");

    // Its worker's terminal is closed under it. A look inside one tick's
    // chance leaves it to the fleet; after that, the manual fix is named.
    world.with_orca(|orca| orca["terminals"] = json!([]));
    let answer = doctor(&world, &now).ok();
    assert_eq!(answer["claims"][0]["worker"]["state"], "gone");
    assert!(found(&answer, "the claim on #3 has no worker").is_empty());
    let tick = &answer["fleet"]["last_tick"];
    let now =
        (at + tick["sleep"].as_u64().unwrap() + tick["took"].as_u64().unwrap_or(0)).to_string();
    let answer = doctor(&world, &now).ok();
    let gone = found(&answer, "the claim on #3 has no worker");
    assert_eq!(gone[0]["severity"], "warning");
    assert_eq!(
        gone[0]["fix"],
        format!("afkconvoy dispatch -C {} --issue 3 --instance alpha", quoted(&world))
    );

    // killed, it is seen dead at once, and what it logged is all still there
    let logged = log(&world);
    drop(running);
    let unwell = doctor(&world, &now);
    assert_eq!(unwell.json["fleet"]["alive"], false);
    assert_eq!(unwell.code, 1, "claims are held and no run is alive");
    assert_eq!(log(&world), logged);
}

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
max_rework: 1
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: done, reject: build }
```
";

/// A world whose repo is bound to a team with the one workflow `feature` →
/// it, and the checkouts of the team's repo and of the role `dev`'s.
fn bound() -> (World, PathBuf, PathBuf) {
    let world = World::new();
    let dev = world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    world.repo(
        "acme/reviewer",
        &[(".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n")],
    );
    let team = world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    (world, team, dev)
}

#[test]
fn a_bound_repos_claim_says_its_stage_and_role_and_a_head_that_moved_since_is_said() {
    let (world, team, dev) = bound();
    world.issue(3, &["flow:feature"]);
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    let answer = doctor(&world, "1100").json;
    let claim = &answer["claims"][0];
    assert_eq!((&claim["stage"], &claim["role"]), (&json!("build"), &json!("dev")));
    assert_eq!(claim["status"], "at_stage");
    assert_eq!(found(&answer, "moved since").len(), 0, "{answer}");

    commit(&dev, "CLAUDE.md", "be careful\n");
    // Its rules alone changed: the team moved all the same.
    commit(&team, "CLAUDE.md", "review before you land\n");
    let answer = doctor(&world, "1100").json;
    let moved = found(&answer, "moved since the stage `build` of #3 started");
    let which: Vec<&str> =
        moved.iter().map(|m| m["what"].as_str().unwrap().split(" moved").next().unwrap()).collect();
    assert_eq!(which, ["the team", "the role `dev`"]);
    let check = format!("afkconvoy check {}", quoted(&world));
    assert!(moved.iter().all(|m| m["severity"] == "info" && m["fix"] == check));
    let head = git(&dev, &["rev-parse", "HEAD"]);
    assert!(moved[1]["evidence"].as_str().unwrap().ends_with(&format!("the head is {head}")));
}

/// What `answer` says moved since the stage `stage` of #3 started, each
/// with its evidence.
fn moved(answer: &Value, stage: &str) -> Vec<(String, String)> {
    let said = found(answer, &format!("moved since the stage `{stage}` of #3 started"));
    let text = |finding: &Value, key: &str| finding[key].as_str().unwrap().to_string();
    let which = |finding: &Value| text(finding, "what").split(" moved").next().unwrap().to_string();
    said.iter().map(|finding| (which(finding), text(finding, "evidence"))).collect()
}

#[test]
fn in_a_repo_of_several_roles_a_role_moved_only_when_its_own_directory_changed() {
    let world = World::new();
    let roles = "```yaml\nroles:\n  fellow: ./roles/fellow\n  attending: ./roles/attending\n```\n";
    let fix = FEATURE.replace("by: dev", "by: fellow").replace("by: reviewer", "by: attending");
    let hospital = world.repo(
        "acme/hospital",
        &[
            ("team.md", roles),
            ("workflows/fix.md", &fix),
            (
                "roles/fellow/.claude/skills/implement/SKILL.md",
                "---\nname: implement\ndescription: build\n---\n",
            ),
            (
                "roles/attending/.claude/skills/review/SKILL.md",
                "---\nname: review\ndescription: review\n---\n",
            ),
        ],
    );
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/hospital\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.issue(3, &["flow:fix"]);
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    let started = git(&hospital, &["rev-parse", "HEAD"]);
    let said = |head: &str| format!("it started under {started}; the head is {head}");
    let answer = doctor(&world, "1100").json;
    assert_eq!(answer["claims"][0]["role"], "fellow", "{answer}");

    // another role's directory, and what is neither a role's nor the team's
    commit(&hospital, "roles/attending/CLAUDE.md", "be strict\n");
    commit(&hospital, ".claude/settings.json", "{}\n");
    assert_eq!(moved(&doctor(&world, "1100").json, "build"), []);

    // its own: the role moved, and the team did not
    let head = commit(&hospital, "roles/fellow/CLAUDE.md", "be careful\n");
    let role = ("the role `fellow`".to_string(), said(&head));
    assert_eq!(moved(&doctor(&world, "1100").json, "build"), std::slice::from_ref(&role));

    // a workflow: the team moved too
    let head = commit(&hospital, "workflows/fix.md", &format!("{fix}\nSaid again.\n"));
    let role = (role.0, said(&head));
    assert_eq!(
        moved(&doctor(&world, "1100").json, "build"),
        [("the team".to_string(), said(&head)), role.clone()]
    );

    // put back as the stage started under them, both are where they were
    git(&hospital, &["rm", "-q", "roles/fellow/CLAUDE.md"]);
    let head = commit(&hospital, "workflows/fix.md", &fix);
    assert_ne!(head, started);
    assert_eq!(moved(&doctor(&world, "1100").json, "build"), []);

    // with no copy of the commit the stage started under, the commits are all there is
    let copies = world.cache().join("afkconvoy/copies/acme/hospital");
    std::fs::rename(copies.join(&started), copies.join("gone")).unwrap();
    let both: Vec<String> =
        moved(&doctor(&world, "1100").json, "build").into_iter().map(|(which, _)| which).collect();
    assert_eq!(both, ["the team", "the role `fellow`"]);
}

#[test]
fn a_team_with_a_fault_and_a_flow_label_that_names_no_workflow_are_findings() {
    let (world, team, _) = bound();
    world.issue(3, &["flow:feature"]);
    world.issue(7, &["flow:bogus"]);
    let answer = doctor(&world, "1000").ok();
    let lost = found(&answer, "#7 follows no workflow of the team");
    assert_eq!(lost[0]["fix"], format!("afkconvoy flows {}", quoted(&world)));
    assert!(
        lost[0]["evidence"]
            .as_str()
            .unwrap()
            .contains("it carries flow:bogus; the team's workflows take flow:feature")
    );
    assert_eq!(found(&answer, "#3").len(), 0, "{answer}");

    // the team's head gets a fault: a stage performed by nobody the team names
    commit(&team, "workflows/feature.md", &FEATURE.replace("by: reviewer", "by: nobody"));
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let fault = found(&unwell.json, "the team acme/team has a fault at its head");
    assert_eq!(fault[0]["severity"], "critical");
    assert_eq!(fault[0]["fix"], "workflows/feature.md: stages[1].by", "{}", unwell.json);
}

#[test]
fn an_issue_long_at_its_stage_and_one_rework_from_its_breaker_are_findings() {
    let (world, ..) = bound();
    world.issue(3, &["flow:feature"]);
    let human = |stage: &str, verdict: &str| {
        let args = ["verdict", verdict, "--issue", "3", "--stage", stage, "--text", "by hand"];
        world.afk(&args).ok();
    };
    // built, rejected once — all `max_rework` allows — and at `build` again
    human("build", "pass");
    human("review", "reject");
    world.with_gh(|gh| {
        let rows = gh["comments"]["3"].as_array_mut().unwrap();
        let records =
            rows.iter_mut().filter(|row| row["body"].as_str().unwrap().contains("convoy:stage"));
        for (row, at) in records.zip([1000, 1600]) {
            row["at"] = json!(at);
        }
    });
    let answer = doctor(&world, "1700").ok();
    let near = found(&answer, "#3 is one rework from a breaker");
    assert_eq!(
        near[0]["fix"],
        format!("afkconvoy show -C {} --issue 3", quoted(&world)),
        "{answer}"
    );
    assert_eq!(found(&answer, "longer than its baseline").len(), 0, "100s in: {answer}");

    // an hour on, it has been at `build` longer than the 10m `build` took before
    let answer = doctor(&world, "5200").ok();
    let long = found(&answer, "#3 has been at the stage `build` for 1h 0m");
    assert_eq!(long[0]["severity"], "warning", "{answer}");
    assert_eq!(long[0]["fix"], format!("afkconvoy show -C {} --issue 3", quoted(&world)));
}

#[test]
fn a_lock_that_cannot_be_worked_is_critical() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(9, &["fleet-lock"]);
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let lock = found(&unwell.json, "the fleet lock #9 stands and cannot be worked");
    assert_eq!(lock[0]["fix"], "gh issue edit 9 --add-label ready-for-agent");
    assert_eq!(lock[0]["see"], "afkconvoy help humans", "a human gives it what it lacks");
    assert_eq!(unwell.json["issues"]["locks"], json!([9]));
    // with the label on it, it is on the frontier and nothing is wrong
    world.with_gh(|gh| gh["issues"][1]["labels"] = json!(["fleet-lock", "ready-for-agent"]));
    assert_eq!(doctor(&world, "1000").ok()["findings"], json!([]));
}

#[test]
fn a_worktree_kept_with_no_claim_is_named_with_what_it_holds_and_how_to_remove_it() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    let tick = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    world.afk(&[&tick[..], &[]].concat()).ok();
    let orca = world.orca();
    let of = |issue: u64| {
        let rows = orca["worktrees"].as_array().unwrap();
        let row = rows.iter().find(|row| row["linkedIssue"] == issue).unwrap();
        PathBuf::from(row["path"].as_str().unwrap())
    };
    let (seven, eight) = (of(7), of(8));
    // #7 holds a commit only it has; #8 held only what an older release kept it for
    commit(&seven, "work.txt", "x\n");
    write(&eight, "scratch.txt", "x");
    for issue in ["7", "8"] {
        let args = ["escalate", "--issue", issue, "--instance", "alpha", "--reason", "stuck"];
        assert_eq!(world.afk(&[&args[..], &["--now", "1100"]].concat()).ok()["worktree"], "kept");
    }
    std::fs::remove_file(eight.join("scratch.txt")).unwrap();

    let answer = doctor(&world, "1200").json;
    let kept = found(&answer, "is kept with no claim");
    assert_eq!(kept.len(), 2, "{answer}");
    let of_issue = |issue: &str| kept.iter().find(|f| f["what"].as_str().unwrap().contains(issue));
    let seven_found = of_issue("#7").unwrap();
    assert_eq!(seven_found["severity"], "info");
    assert!(
        seven_found["what"].as_str().unwrap().ends_with("1 commit the remote does not have"),
        "{seven_found}"
    );
    let fix = seven_found["fix"].as_str().unwrap();
    let remove =
        |dir: &PathBuf| format!("orca worktree rm --worktree path:{} --force", dir.display());
    assert!(fix.contains(&format!("git -C {} push origin HEAD:", seven.display())), "{fix}");
    assert!(fix.ends_with(&remove(&seven)), "{fix}");
    let eight_found = of_issue("#8").unwrap();
    assert!(eight_found["what"].as_str().unwrap().ends_with("nothing the remote does not"));
    assert_eq!(eight_found["fix"], remove(&eight), "nothing to push first");
}

#[test]
fn a_claim_on_a_deleted_issue_is_a_finding_with_the_line_that_deletes_it() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world
        .afk(&["claim", "--issue", "3", "--instance", "alpha", "--host", "h2", "--now", "1000"])
        .ok();
    world.with_gh(|gh| {
        gh["issues"] = json!([]);
        gh["deleted"] = json!({"3": 0});
    });
    let answer = doctor(&world, "1100").json;
    let on_gone = found(&answer, "is claimed but GitHub has it no more");
    assert_eq!(on_gone.len(), 1, "{answer}");
    assert_eq!(on_gone[0]["severity"], "warning");
    assert_eq!(on_gone[0]["fix"], "git push origin :refs/convoy/claim/3");
    assert!(on_gone[0]["evidence"].as_str().unwrap().contains("held by alpha"), "{answer}");
}

/// Append the older releases' actual on-disk shape as well as current lines:
/// the watching command must interpret the log, not a test-only event source.
fn append_log(world: &World, lines: &[Value]) {
    use std::io::Write;
    let answer = doctor(world, "900").json;
    let file = answer["fleet"]["log"].as_str().unwrap();
    let mut file = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

fn logged_command(at: u64, issue: Option<u64>, command: &str, outcome: &str) -> Value {
    json!({"at": at, "instance": "", "host": "h1", "command": {
        "command": command, "on": issue.map(|n| format!("#{n}")),
        "by": "worker", "outcome": outcome}})
}

#[test]
fn watching_combines_gate_progress_github_endings_and_adjacent_look_boundaries() {
    let world = World::new();
    world.list_checkout(&world.clone);
    for n in [3, 5, 7, 8, 9] {
        world.issue(n, &[]);
    }
    run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "900"]);
    world.pr(101, 3, "work3");
    world.pr(102, 5, "work5");
    world.pr(103, 7, "work7");
    world.fork_pr(104, 8, "fork8");
    world.with_gh(|gh| {
        gh["issues"][1]["state"] = json!("closed");
        gh["issues"][1]["closed_at"] = json!(1040);
        gh["prs"][1]["state"] = json!("merged");
        gh["prs"][1]["merged_at"] = json!(1041);
        gh["prs"][1]["closed_at"] = json!(1041);
        gh["prs"][1]["updated_at"] = json!(1041);
        gh["prs"][2]["state"] = json!("closed");
        gh["prs"][2]["closed_at"] = json!(1042);
        gh["prs"][2]["updated_at"] = json!(1042);
        gh["prs"][3]["state"] = json!("merged");
        gh["prs"][3]["merged_at"] = json!(1043);
        gh["prs"][3]["updated_at"] = json!(1043);
        gh["comments"]["7"] = json!([{"id": 701, "at": 1030,
            "body": "<!--convoy:intake workflow=feature by=train-->", "html_url": "intake7"}]);
        gh["comments"]["9"] = json!([{"id": 901, "at": 1100,
            "body": "<!--convoy:intake workflow=feature by=train-->", "html_url": "intake9"}]);
    });
    append_log(
        &world,
        &[
            logged_command(950, Some(3), "gate", "started"),
            logged_command(1000, Some(3), "exclusive", "waits behind 6 exclusive commands"),
            logged_command(1020, Some(3), "exclusive", "runs (waited 633s)"),
            logged_command(1040, Some(3), "exclusive", "exit 1 in 122s"),
            logged_command(1040, Some(3), "gate", "red"),
            logged_command(1045, Some(3), "land", "conflict"),
            logged_command(1099, Some(3), "exclusive", "waits behind 4 exclusive commands"),
            logged_command(1100, Some(8), "dispatch", "done"),
        ],
    );
    world.gh_calls();
    let args = ["doctor", "--all", "--host", "h1", "--since", "1000", "--now", "1100"];
    let answer = world.afk(&args).ok();
    assert_eq!(answer["until"], 1100);
    let watching = &answer["checkouts"][0]["doctor"]["watching"];
    let issue = |number| {
        watching["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|issue| issue["issue"] == number)
            .unwrap()
    };
    let steps = issue(3)["steps"].as_array().unwrap();
    let names: Vec<_> =
        steps.iter().map(|s| (s["at"].as_u64().unwrap(), s["step"].as_str().unwrap())).collect();
    assert_eq!(
        names,
        [
            (1000, "exclusive"),
            (1020, "exclusive"),
            (1040, "exclusive"),
            (1040, "gate"),
            (1045, "land"),
            (1099, "exclusive")
        ]
    );
    assert_eq!(issue(3)["pr"], 101);
    assert_eq!(issue(3)["last_step"]["ago"], 1);
    let ended: Vec<_> =
        watching["ended"].as_array().unwrap().iter().map(|s| s["step"].as_str().unwrap()).collect();
    assert_eq!(ended, ["intake", "issue_closed", "pr_merged", "pr_closed"]);
    assert!(
        watching["issues"].as_array().unwrap().iter().all(|s| s["issue"] != 8 && s["issue"] != 9)
    );
    for call in world.gh_calls() {
        assert!(
            !call
                .iter()
                .any(|a| ["POST", "PATCH", "PUT", "DELETE", "edit", "create", "close", "comment"]
                    .contains(&a.as_str())),
            "{call:?}"
        );
    }
    // More lines arrive in the same second as the preceding boundary. Both
    // belong to the next look, along with GitHub's intake stamped then.
    append_log(&world, &[logged_command(1100, Some(8), "gate", "started")]);
    let second =
        world.afk(&["doctor", "--all", "--host", "h1", "--since", "1100", "--now", "1200"]).ok();
    let next = &second["checkouts"][0]["doctor"]["watching"];
    let rows = next["issues"].as_array().unwrap();
    assert_eq!(rows.iter().find(|s| s["issue"] == 3).unwrap()["steps"], json!([]));
    assert_eq!(
        rows.iter().find(|s| s["issue"] == 8).unwrap()["steps"].as_array().unwrap().len(),
        2
    );
    assert_eq!(next["ended"].as_array().unwrap().len(), 1);
    assert_eq!(next["ended"][0]["issue"], 9);
    // First looks still give the original state, with a boundary to start at.
    let first = world.afk(&["doctor", "--all", "--now", "1200"]).ok();
    assert_eq!(first["until"], 1200);
    assert!(first["checkouts"][0]["doctor"].get("watching").is_none());
}

#[test]
fn watching_pages_prs_and_comments_and_withholds_until_when_a_source_fails() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &[]);
    world.with_gh(|gh| {
        // An intake beyond the comments returned in the issue query.
        let mut comments: Vec<Value> = (0..101)
            .map(|n| {
                json!({"id": n + 1,
            "at": 900 + n, "body": "ordinary comment", "html_url": "comment"})
            })
            .collect();
        comments[100]["body"] = json!("<!--convoy:intake workflow=feature by=train-->");
        gh["comments"]["3"] = json!(comments);
        let prs: Vec<Value> = (0..101)
            .map(|n| {
                json!({"number": 200 + n,
            "title": "ended", "body": "", "headRefName": "work", "headRefOid": "",
            "state": "closed", "closed_at": 1000, "updated_at": 1000})
            })
            .collect();
        gh["prs"] = json!(prs);
    });
    let args =
        ["doctor", "--all", "--host", "h1", "--since", "1970-01-01T00:16:40Z", "--now", "1100"];
    let answer = world.afk(&args).ok();
    let ended = answer["checkouts"][0]["doctor"]["watching"]["ended"].as_array().unwrap();
    assert_eq!(ended.len(), 102, "the intake and every PR, including page two");
    for failure in ["since=", "pulls?", "/comments"] {
        world.with_gh(|gh| gh["fail"] = json!([failure]));
        let failed = world.afk(&args);
        assert_eq!(failed.code, 1, "{}", failed.json);
        assert_eq!(failed.json["until"], Value::Null);
        assert!(failed.json["checkouts"][0]["doctor"]["watching"]["error"].is_string());
        assert!(failed.json["checkouts"][0]["doctor"]["claims"].is_array());
    }
    let future = world.afk(&["doctor", "--all", "--since", "1200", "--now", "1100"]);
    assert_eq!(future.code, 1);
    assert!(future.json["error"].as_str().unwrap().contains("--since is after"));
}

/// Names `harness` on `checkout` in this world's checkout list.
fn name_harness(world: &World, checkout: &std::path::Path, harness: &str) {
    let file = world.cache().join("afkconvoy/checkouts.json");
    let mut list: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    for entry in list["checkouts"].as_array_mut().unwrap() {
        if entry["checkout"] == json!(checkout) {
            entry["harness"] = json!(harness);
        }
    }
    std::fs::write(file, list.to_string()).unwrap();
}

fn clear_shell(world: &World) {
    std::fs::write(world.root.join("shell-asked"), "").unwrap();
    std::fs::write(world.root.join("shell-asked-in"), "").unwrap();
}

fn issue_queries_in(doc: &Value) -> Vec<String> {
    doc["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|call| {
            call.as_array().unwrap().iter().find_map(|arg| {
                arg.as_str()
                    .and_then(|arg| arg.strip_prefix("query="))
                    .filter(|query| query.contains("issues("))
                    .map(str::to_string)
            })
        })
        .collect()
}

fn starts_a_harness(script: &str) -> bool {
    script.contains("type --")
        || script.starts_with("kimi ")
        || script.starts_with("grok ")
        || script.starts_with("claude ")
        || script.starts_with("pi ")
        || script.contains("codex ")
}

#[test]
fn a_look_reads_the_login_once_and_starts_no_harness_while_a_probe_stands() {
    let world = World::new();
    world.hold_every_harness();
    let second = world.second_repo();
    name_harness(&world, &world.clone, "kimi");
    name_harness(&world, &second, "kimi");
    world.afk(&["plan", "--now", "1000"]).ok();
    world.afk_in(&second, &["plan", "--now", "1000"]).ok();

    let look = |now: &str| {
        clear_shell(&world);
        let _ = world.gh_calls();
        let mut gh2: Value =
            serde_json::from_slice(&std::fs::read(world.root.join("gh2.json")).unwrap()).unwrap();
        gh2["calls"] = json!([]);
        std::fs::write(world.root.join("gh2.json"), gh2.to_string()).unwrap();

        let answer = world.afk(&["doctor", "--all", "--now", now]).ok();
        assert_eq!(answer["checkouts"].as_array().unwrap().len(), 2, "{answer}");
        let asked = world.shell_asked();
        let auth: Vec<_> =
            asked.iter().filter(|script| script.starts_with("gh auth status")).collect();
        assert_eq!(auth.len(), 1, "{asked:?}");
        let started: Vec<_> = asked.iter().filter(|script| starts_a_harness(script)).collect();
        assert!(started.is_empty(), "{asked:?}");

        let calls = world.gh_calls();
        assert_eq!(
            issue_queries_of(&calls),
            vec![afkconvoy::gh::OPEN_ISSUES_QUERY.to_string()],
            "{calls:?}"
        );
        for call in &calls {
            assert!(
                !call.iter().any(|arg| {
                    ["POST", "PATCH", "PUT", "DELETE", "edit", "create", "close", "comment"]
                        .contains(&arg.as_str())
                }),
                "{call:?}"
            );
        }
        let gh2: Value =
            serde_json::from_slice(&std::fs::read(world.root.join("gh2.json")).unwrap()).unwrap();
        let second_queries = issue_queries_in(&gh2);
        assert_eq!(
            second_queries,
            vec![afkconvoy::gh::OPEN_ISSUES_QUERY.to_string()],
            "{second_queries:?}"
        );
    };
    look("1200");
    look("2000");
}

fn issue_queries_of(calls: &[Vec<String>]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|call| {
            call.iter().find_map(|arg| {
                arg.strip_prefix("query=")
                    .filter(|query| query.contains("issues("))
                    .map(str::to_string)
            })
        })
        .collect()
}

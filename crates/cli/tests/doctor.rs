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

/// The lines of the log this world's `run` kept.
fn log(world: &World) -> Vec<Value> {
    let runs = world.cache().join("afkconvoy/runs");
    let files = std::fs::read_dir(&runs).unwrap().map(|entry| entry.unwrap().path());
    // Beside it, the lock its writers take.
    let files: Vec<PathBuf> =
        files.filter(|file| file.extension().is_some_and(|ext| ext == "jsonl")).collect();
    assert_eq!(files.len(), 1, "one log for the one repo: {files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
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
fn a_fleet_with_no_fault_answers_its_keys_and_no_finding_and_writes_nothing() {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
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
    assert_eq!(keys, ["claims", "findings", "fleet", "host", "issues", "waiting_on_human"]);
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
    assert_eq!(fleet["last_tick"]["line"], "dispatched #3 · holding 1/2 · frontier 0");
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
    assert_eq!(found(&answer, "no `afkconvoy run` is alive")[0]["fix"], "afkconvoy run");
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
    assert_eq!(answer["host"]["exclusive"], json!({"running": null, "waiting": []}));
}

#[test]
fn the_host_s_exclusive_queue_is_shown_what_runs_first_then_what_waits() {
    use afkconvoy::exclusive::Queue;
    use afkconvoy_core::exclusive::Ticket;
    let world = World::new();
    let queue = Queue::at(world.cache().join("afkconvoy/exclusive"));
    let ticket = |command: &str, issue| Ticket {
        number: 0,
        pid: 42,
        command: command.into(),
        dir: "/w/acme".into(),
        issue,
        asked: 900,
        ran: None,
    };
    let (mut running, _) = queue.join(ticket("cargo test", Some(12))).unwrap();
    running.begin(950).unwrap();
    let (_waiting, _) = queue.join(ticket("make bench", None)).unwrap();
    let exclusive = &doctor(&world, "1000").ok()["host"]["exclusive"];
    let shown = |command: &str, issue: Value, ran: Value| {
        json!({"command": command, "dir": "/w/acme", "issue": issue, "pid": 42, "asked": 900,
            "ran": ran})
    };
    assert_eq!(exclusive["running"], shown("cargo test", json!(12), json!(950)));
    assert_eq!(exclusive["waiting"], json!([shown("make bench", Value::Null, Value::Null)]));
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
    assert_eq!(finding["fix"], "afkconvoy run");
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
    world.say_concurrency(&world.clone, 2);
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
    let host_sleep = "start: puts this host to sleep";
    let printed: Vec<&String> = lines.iter().filter(|line| !line.starts_with(host_sleep)).collect();
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
        assert_eq!(tick["bound"], 2);
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
    let said: Vec<(&Value, &Value, &Value)> =
        events.iter().map(|event| (&event["at"], &event["event"], &event["instance"])).collect();
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
    assert_eq!(failed[0]["fix"], "afkconvoy plan --instance alpha");
    let steps = unwell.json["fleet"]["failed_steps"].as_array().unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!((&steps[0]["tick"], &steps[0]["do"]), (&json!(3), &json!("tick")));
}

#[test]
fn a_step_that_fails_tick_after_tick_is_critical() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.with_orca(|orca| orca["create_fails"] = json!([3]));
    // the first tick fails as it starts the worker, each one after as it looks for it
    run(&world, &["--instance", "alpha", "--ticks", "4", "--now", "1000"]);
    let unwell = doctor(&world, "1000");
    assert_eq!(unwell.code, 1);
    let failed = found(&unwell.json, "`judge #3` failed in each of the last 3 ticks (2 to 4)");
    assert_eq!(failed[0]["severity"], "critical", "{}", unwell.json);
    assert_eq!(unwell.json["fleet"]["failed_steps"].as_array().unwrap().len(), 4);
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
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1"];
    let child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let running = Running(child);
    // its first tick is over once its log says so
    let started = Instant::now();
    let runs = world.cache().join("afkconvoy/runs");
    while !runs.is_dir() || ticks(&log(world)).is_empty() {
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

    // orca has no report of it — a worker in its role loads none of the
    // user's hooks: one writing to its terminal is at work all the same
    world.with_orca(|orca| orca["terminals"][0]["agent"] = json!(null));
    let answer = doctor(&world, &now).ok();
    assert_eq!(answer["claims"][0]["worker"]["state"], "busy");
    world.with_orca(|orca| orca["terminals"][0]["lastOutputAt"] = json!((at - 400) * 1000));
    let answer = doctor(&world, &now).ok();
    assert_eq!(answer["claims"][0]["worker"], json!({"state": "stopped", "quiet": 400}));

    // its worker's terminal is closed under it
    world.with_orca(|orca| orca["terminals"] = json!([]));
    let answer = doctor(&world, &now).ok();
    assert_eq!(answer["claims"][0]["worker"]["state"], "gone");
    let gone = found(&answer, "the claim on #3 has no worker");
    assert_eq!(gone[0]["severity"], "warning");
    assert_eq!(gone[0]["fix"], "afkconvoy dispatch --issue 3 --instance alpha");

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
    assert!(moved.iter().all(|m| m["severity"] == "info" && m["fix"] == "afkconvoy check"));
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
    assert_eq!(lost[0]["fix"], "afkconvoy flows");
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
    assert_eq!(near[0]["fix"], "afkconvoy show --issue 3", "{answer}");
    assert_eq!(found(&answer, "longer than any stage").len(), 0, "100s in: {answer}");

    // an hour on, it has been at `build` longer than the 10m `build` took before
    let answer = doctor(&world, "5200").ok();
    let long = found(&answer, "#3 has been at the stage `build` for 1h 0m");
    assert_eq!(long[0]["severity"], "warning", "{answer}");
    assert_eq!(long[0]["fix"], "afkconvoy show --issue 3");
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

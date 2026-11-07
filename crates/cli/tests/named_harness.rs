//! A checkout names the harness its workers start on. The name is that
//! checkout's entry on this host's checkout list. Another checkout on the
//! same host still follows the host kit. The host's watch never does.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::common::{self, World, git};

/// A `run` whose sleeps are short, so the next tick of a fleet that holds
/// claims comes within a second.
const TICKING: [&str; 5] = ["run", "--host", "h1", "--second-ms", "10"];

const KIMI_SCREEN: &str = include_str!("screens/kimi-5-hour-usage-limit-control-166.txt");
const CODEX_REFUSAL: &str = include_str!("screens/codex-task-refusal-control-98.txt");

fn name(world: &World, checkout: &Path, harness: &str) {
    let file = world.cache().join("afkconvoy/checkouts.json");
    let mut list = world.checkout_list();
    let entries = list["checkouts"].as_array_mut().unwrap();
    match entries.iter_mut().find(|entry| entry["checkout"] == json!(checkout)) {
        Some(entry) => entry["harness"] = json!(harness),
        None => entries.push(json!({"checkout": checkout, "harness": harness})),
    }
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, list.to_string()).unwrap();
}

fn harness_of(world: &World, checkout: &Path) -> Value {
    let list = world.checkout_list();
    let rows = list["checkouts"].as_array().unwrap();
    rows.iter()
        .find(|row| row["checkout"] == json!(checkout))
        .map(|row| row["harness"].clone())
        .unwrap_or(Value::Null)
}

fn applied(world: &World, checkout: &Path) -> Option<String> {
    let list = world.checkout_list();
    let rows = list["checkouts"].as_array().unwrap();
    rows.iter()
        .find(|row| row["checkout"] == json!(checkout))
        .and_then(|row| row["harness_applied"].as_str().map(str::to_string))
}

fn worktree(world: &World, issue: u64) -> Option<PathBuf> {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array()?;
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue)?;
    Some(PathBuf::from(row["path"].as_str()?))
}

fn open_commands(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|terminal| {
        terminal["worktreePath"] == dir.to_str().unwrap() && terminal["open"] == true
    });
    here.map(|terminal| terminal["command"].as_str().unwrap_or("").to_string()).collect()
}

/// Waits until issue `issue` has one open terminal whose command contains
/// `needle` → that command.
fn wait_command(world: &World, issue: u64, needle: &str) -> String {
    let until = Instant::now() + Duration::from_secs(40);
    loop {
        if let Some(dir) = worktree(world, issue) {
            let commands = open_commands(world, &dir);
            if commands.len() == 1 && commands[0].contains(needle) {
                return commands[0].clone();
            }
        }
        assert!(
            Instant::now() < until,
            "issue {issue} did not start with {needle:?}: {:?}",
            worktree(world, issue).map(|dir| open_commands(world, &dir))
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

fn issue_on_second(world: &World, number: u64) {
    let mut gh = world.gh2();
    gh["issues"].as_array_mut().unwrap().push(json!({
        "number": number,
        "title": format!("issue {number}"),
        "labels": ["ready-for-agent"],
        "state": "open"
    }));
    std::fs::write(world.root.join("gh2.json"), gh.to_string()).unwrap();
}

fn row<'a>(answer: &'a Value, checkout: &Path) -> &'a Value {
    answer["checkouts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["checkout"] == json!(checkout))
        .unwrap_or_else(|| panic!("no row of {} in {answer}", checkout.display()))
}

#[test]
fn a_running_checkout_names_kimi_and_another_checkout_stays_on_the_host_kit() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_researched_models("kimi", &["kimi-code/k3", "kimi-code/k2"]);
    world.issue(7, &["ready-for-agent"]);
    let first = world.afk(&TICKING).ok();
    let claude = wait_command(&world, 7, "agent");
    assert!(!claude.contains("kimi --auto"), "{claude}");

    let named = world.afk(&[&TICKING[..], &["--harness", "kimi"]].concat()).ok();
    assert_eq!(named["added"], false, "{named}");
    assert_eq!(named["instance"], first["instance"], "{named}");
    assert_eq!(named["host_runner"]["pid"], first["host_runner"]["pid"]);
    assert_eq!(
        named["harness"],
        json!({
            "from": {"in_force": "claude-code", "said_by": "host_kit"},
            "to": {"in_force": "kimi", "said_by": "checkout"}
        }),
        "{named}"
    );
    assert_eq!(harness_of(&world, &world.clone), json!("kimi"));

    let on_kimi = wait_command(&world, 7, "kimi --auto");
    assert!(on_kimi.contains("kimi --auto"), "{on_kimi}");
    assert!(!on_kimi.contains("--model"), "no model label, so the user default: {on_kimi}");

    world.issue(8, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent", "model:kimi-code/k2"]);
    let unlabelled = wait_command(&world, 8, "kimi --auto");
    assert!(!unlabelled.contains("--model"), "no model label, so the user default: {unlabelled}");
    assert!(unlabelled.contains("kimi --auto"), "{unlabelled}");
    let labelled = wait_command(&world, 9, "--model 'kimi-code/k2'");
    assert!(labelled.contains("kimi --auto"), "{labelled}");
    // A path in the command may hold the word, as a checkout under
    // `.claude/worktrees` does: the program it runs may not.
    assert!(!labelled.split(" && ").any(|part| part.starts_with("claude")), "{labelled}");

    let other = world.second_repo();
    issue_on_second(&world, 11);
    let second = world.afk_in(&other, &TICKING).ok();
    assert_eq!(second["harness"], Value::Null, "it named none: {second}");
    assert_ne!(second["instance"], first["instance"]);
    let followed = wait_command(&world, 11, "agent");
    assert_eq!(followed, common::trusted("agent"), "{followed}");
    assert_eq!(harness_of(&world, &other), Value::Null);

    let listed = world.afk(&["checkouts"]).ok();
    assert_eq!(
        row(&listed, &world.clone)["harness"],
        json!({"in_force": "kimi", "said_by": "checkout"})
    );
    assert_eq!(
        row(&listed, &other)["harness"],
        json!({"in_force": "claude-code", "said_by": "host_kit"})
    );
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["harness"], json!({"in_force": "kimi", "said_by": "checkout"}));
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]);
    assert_eq!(doctor.json["fleet"]["harness"], plan["harness"]);
}

#[test]
fn a_worker_already_at_work_is_continued_once_and_switch_does_not_move_it() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_researched_models("kimi", &["kimi-code/k3"]);
    world.issue(7, &["ready-for-agent"]);
    let first = world.afk(&TICKING).ok();
    let dir = loop {
        if let Some(dir) = worktree(&world, 7) {
            break dir;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    wait_command(&world, 7, "agent");
    let head = git(&dir, &["rev-parse", "HEAD"]);
    world.orca_calls();

    world.afk(&[&TICKING[..], &["--harness", "kimi"]].concat()).ok();
    let moved = wait_command(&world, 7, "kimi --auto");
    assert!(moved.contains("kimi --auto"), "{moved}");
    assert!(
        !moved.contains("--model"),
        "one model in the kit still does not name the start: {moved}"
    );
    assert_eq!(open_commands(&world, &dir).len(), 1, "one writer");
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(world.claims().len(), 1);
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    let close = calls.iter().position(|call| call == "terminal close");
    let create = calls.iter().position(|call| call == "terminal create");
    assert!(close.is_some() && create.is_some() && close < create, "{calls:?}");
    assert!(
        !calls.iter().any(|call| call == "worktree rm" || call == "worktree create"),
        "{calls:?}"
    );
    let until = Instant::now() + Duration::from_secs(5);
    while applied(&world, &world.clone).as_deref() != Some("kimi") {
        assert!(Instant::now() < until, "the converge was not recorded: {}", world.checkout_list());
        std::thread::sleep(Duration::from_millis(50));
    }

    // A later tick does not start a second writer on the named harness.
    world.orca_calls();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(open_commands(&world, &dir), vec![moved.clone()]);
    let again: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!again.iter().any(|call| call == "terminal create"), "{again:?}");

    let plain = world.afk(&["switch", "--harness", "pi", "--host", "h1", "--now", "2000"]).ok();
    assert_eq!(plain["order"][0], "pi");
    assert_eq!(plain["switched"], json!([]));
    assert_eq!(harness_of(&world, &world.clone), json!("kimi"));
    assert_eq!(open_commands(&world, &dir), vec![moved.clone()]);

    let all =
        world.afk(&["switch", "--all", "--harness", "grok", "--host", "h1", "--now", "2100"]).ok();
    assert_eq!(all["order"][0], "grok");
    assert_eq!(all["switched"], json!([]), "{all}");
    assert_eq!(harness_of(&world, &world.clone), json!("kimi"));
    assert!(open_commands(&world, &dir)[0].contains("kimi --auto"));

    let cleared = world.afk(&[&TICKING[..], &["--harness", "default"]].concat()).ok();
    assert_eq!(cleared["instance"], first["instance"]);
    assert_eq!(
        cleared["harness"]["to"],
        json!({"in_force": "grok", "said_by": "host_kit"}),
        "{cleared}"
    );
    assert_eq!(harness_of(&world, &world.clone), Value::Null);
    let followed = wait_command(&world, 7, "grok --always-approve");
    assert!(followed.contains("grok"), "{followed}");
    assert!(!followed.contains("--model"), "no model label, so the user default: {followed}");
    let until = Instant::now() + Duration::from_secs(5);
    while applied(&world, &world.clone).as_deref() != Some("") {
        assert!(Instant::now() < until, "the clear was not recorded: {}", world.checkout_list());
        std::thread::sleep(Duration::from_millis(50));
    }

    // A later tick of a checkout that names none does not move a healthy
    // worker onto the host kit's new first.
    world.with_kit(|kit| {
        let entries = kit["harnesses"].as_array_mut().unwrap();
        let at = entries.iter().position(|held| held["harness"] == "pi").unwrap();
        let held = entries.remove(at);
        entries.insert(0, held);
    });
    std::thread::sleep(Duration::from_secs(2));
    assert!(open_commands(&world, &dir)[0].contains("grok --always-approve"));
    assert_eq!(applied(&world, &world.clone).as_deref(), Some(""));
}

#[test]
fn a_role_that_holds_no_limits_for_the_named_harness_starts_no_worker() {
    let world = World::new();
    world.hold_every_harness();
    world.repo(
        "acme/dev",
        &[
            (
                ".claude/skills/implement/SKILL.md",
                "---\nname: implement\ndescription: build\n---\n",
            ),
            (
                ".claude/settings.json",
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}"#,
            ),
        ],
    );
    world.repo(
        "acme/team",
        &[
            ("team.md", "```yaml\nroles:\n  dev: acme/dev\n```\n"),
            (
                "workflows/feature.md",
                "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    on: { pass: done }\n```\n",
            ),
        ],
    );
    common::write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n",
    );
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    name(&world, &world.clone, "kimi");
    world.issue(7, &["flow:feature"]);

    tick(&world, 700);
    assert!(worktree(&world, 7).is_none(), "no other harness is started");
    assert!(world.claims().is_empty());

    let plan = world.afk(&["plan", "--now", "800"]).ok();
    assert_eq!(plan["harness"], json!({"in_force": "kimi", "said_by": "checkout"}));
    let excluded = plan["excluded"].as_array().unwrap();
    assert_eq!(excluded.len(), 1, "{plan}");
    let reason = excluded[0]["reason"].as_str().unwrap();
    assert!(reason.contains(&world.clone.display().to_string()), "{reason}");
    assert!(reason.contains("kimi") && reason.contains("holds no limits"), "{reason}");
    assert!(plan["frontier"].as_array().unwrap().is_empty(), "{plan}");

    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "801"]);
    let found: Vec<&Value> = doctor.json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| {
            finding["what"]
                .as_str()
                .unwrap()
                .contains("names a harness no worker of this checkout starts on")
        })
        .collect();
    assert_eq!(found.len(), 1, "{}", doctor.json);
    let what = found[0]["what"].as_str().unwrap();
    assert!(what.contains(&world.clone.display().to_string()), "{what}");
    assert!(what.contains("kimi") && what.contains("holds no limits"), "{what}");
    assert!(found[0]["fix"].as_str().unwrap().contains(".kimi-code/limits.toml"), "{}", found[0]);
    assert!(world.open_terminals().is_empty(), "{:?}", world.open_terminals());
}

#[test]
fn an_account_fault_on_the_named_harness_starts_no_worker_on_the_next() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_researched_models("kimi", &["kimi-code/k3"]);
    name(&world, &world.clone, "kimi");
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    let dir = worktree(&world, 7).expect("the named harness started the worker");
    let commands = open_commands(&world, &dir);
    let started = &commands[0];
    assert!(started.contains("kimi --auto"), "{started}");
    assert!(!started.contains("--model"), "no model label, so the user default: {started}");
    let evidence = KIMI_SCREEN.lines().next().unwrap();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({
                "state": "done",
                "outcome": "failure",
                "programStatus": {"state": "error"}
            });
            terminal["screen"] = json!(KIMI_SCREEN.lines().collect::<Vec<_>>());
        }
    });

    tick(&world, 800);
    assert!(
        open_commands(&world, &dir)[0].contains("kimi --auto"),
        "the faulted worker is not continued onto the next harness: {:?}",
        open_commands(&world, &dir)
    );
    assert_eq!(harness_of(&world, &world.clone), json!("kimi"), "a worker fault leaves the name");

    world.issue(8, &["ready-for-agent"]);
    tick(&world, 900);
    assert!(worktree(&world, 8).is_none(), "no worker of this checkout starts on the next harness");
    assert_eq!(world.claims().len(), 1);

    let plan = world.afk(&["plan", "--now", "901"]).ok();
    let reason = plan["excluded"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["number"] == 8)
        .unwrap_or_else(|| panic!("{plan}"))["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(reason.contains("kimi has an account fault") && reason.contains(evidence), "{reason}");
    assert!(reason.contains(&world.clone.display().to_string()), "{reason}");

    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "902"]);
    let what = doctor.json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| {
            finding["what"]
                .as_str()
                .unwrap()
                .contains("names a harness no worker of this checkout starts on")
        })
        .unwrap_or_else(|| panic!("{}", doctor.json))["what"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(what.contains(&world.clone.display().to_string()), "{what}");
    assert!(what.contains("kimi") && what.contains("account fault"), "{what}");
}

#[test]
fn a_task_refusal_on_the_named_harness_continues_on_no_other() {
    let world = World::new();
    world.hold_every_harness();
    world.kit_researched_models("codex", &["gpt-x"]);
    name(&world, &world.clone, "codex");
    world.issue(7, &["ready-for-agent"]);
    tick(&world, 700);
    let dir = worktree(&world, 7).expect("the named harness started the worker");
    let started = open_commands(&world, &dir);
    assert!(started[0].contains("codex"), "{started:?}");
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "done", "outcome": "failure"});
            terminal["screen"] = json!(CODEX_REFUSAL.lines().collect::<Vec<_>>());
        }
    });

    tick(&world, 800);
    let plan = world.afk(&["plan", "--now", "801"]).ok();
    let worker = &plan["selection"]["health"]["workers"][0];
    assert_eq!(worker["diagnosis"]["class"], "refusal", "{plan}");
    let orca = world.orca();
    let opened: Vec<&str> = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|terminal| terminal["open"] == true)
        .map(|terminal| terminal["command"].as_str().unwrap_or(""))
        .collect();
    assert!(
        opened.iter().all(|command| command.contains("codex")),
        "the refused worker is not continued onto another harness: {opened:?}"
    );
    assert_eq!(harness_of(&world, &world.clone), json!("codex"));
}

#[test]
fn a_harness_the_host_kit_does_not_hold_and_a_word_that_is_none_write_nothing() {
    let world = World::new();
    let before = world.checkout_list();
    let unknown = world.afk(&["run", "--host", "h1", "--harness", "nope"]).error();
    assert!(unknown.contains("is no harness") && unknown.contains("`nope`"), "{unknown}");
    assert_eq!(world.checkout_list(), before);

    let missing = world.afk(&["run", "--host", "h1", "--harness", "pi"]).error();
    assert!(missing.contains("pi") && missing.contains("nothing was written"), "{missing}");
    assert!(missing.contains("afkconvoy kit"), "{missing}");
    assert_eq!(harness_of(&world, &world.clone), Value::Null);
    let listed = world.checkout_list();
    assert!(
        listed["checkouts"].as_array().unwrap().iter().all(|row| row.get("harness").is_none()),
        "{listed}"
    );
}

#[test]
fn the_host_watch_starts_from_the_last_brief_and_not_a_checkouts_named_harness() {
    let world = World::new();
    let dir = world.root.join("operator");
    std::fs::create_dir_all(&dir).unwrap();
    world.with_orca(|orca| {
        orca["worktrees"].as_array_mut().unwrap().push(json!({
            "path": dir,
            "repoId": "operator",
            "isMainWorktree": true,
            "isArchived": false
        }));
        orca["terminals"].as_array_mut().unwrap().push(json!({
            "handle": "operators-own",
            "worktreePath": dir,
            "command": "claude",
            "sent": [],
            "open": true
        }));
    });
    world.hold_every_harness();
    world.kit_researched_models("codex", &["gpt-x"]);
    world.kit_researched_models("grok", &["grok-1"]);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    world.pause("pi", now.as_secs(), "it refused every prompt");
    name(&world, &world.clone, "grok");
    let delivered = world.cache().join("afkconvoy/watch/delivered.json");
    std::fs::create_dir_all(delivered.parent().unwrap()).unwrap();
    std::fs::write(&delivered, r#"["codex"]"#).unwrap();

    let watch_dir = dir.to_str().unwrap();
    world
        .afk(&[
            "run",
            "--host",
            "h1",
            "--second-ms",
            "100",
            "--watch-dir",
            watch_dir,
            "--watch-every",
            "1",
        ])
        .ok();
    let until = Instant::now() + Duration::from_secs(60);
    let running = loop {
        let file = world.cache().join("afkconvoy/watch/state.json");
        let state: Value =
            std::fs::read(&file).map_or(json!({}), |bytes| serde_json::from_slice(&bytes).unwrap());
        if !state["running"].is_null() {
            break state["running"].clone();
        }
        assert!(Instant::now() < until, "no watch within 60s: {state}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(running["harness"], "codex", "{running}");
}

#[test]
fn the_glossary_and_adr_0020_say_a_checkout_can_name_its_harness() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let glossary = std::fs::read_to_string(root.join("GLOSSARY.md")).unwrap();
    assert!(glossary.contains("run --harness"), "{glossary}");
    assert!(
        glossary.contains("names no harness starts on the host kit's first eligible one"),
        "{glossary}"
    );
    assert!(glossary.contains("no other harness is started in its place"), "{glossary}");
    assert!(!glossary.contains("docs/adr/"), "{glossary}");
    let yielded = std::fs::read_to_string(
        root.join("docs/adr/0020-an-issue-is-cast-within-its-role-s-range-on-the-host.md"),
    )
    .unwrap();
    assert!(
        yielded.contains("yielded for a checkout that names a harness by ADR-0045"),
        "{yielded}"
    );
    assert!(yielded.contains("never starts by a checkout's named harness"), "{yielded}");
    let recorded = std::fs::read_to_string(
        root.join("docs/adr/0045-a-checkout-can-name-the-harness-its-workers-start-on.md"),
    )
    .unwrap();
    assert!(recorded.contains("ADR-0020"), "{recorded}");
    assert!(recorded.contains("checkout list"), "{recorded}");
    assert!(recorded.contains("--harness default"), "{recorded}");
}

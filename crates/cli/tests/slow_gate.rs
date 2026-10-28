//! A slow gate: a tick that finds the last landings on one host all took most
//! of the gate limit opens one issue for the fleet, and `plan` and `doctor`
//! say so with the same evidence. Whether the gate is slow is the core's own
//! tests'; these are the reads and the opening.

use crate::common;

use afkconvoy::claims::{Me, Scan, take_run};
use afkconvoy::git::Repo;
use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::gate::{GateCommand, Ran, Recording, Tree, after_run, record_ref};
use afkconvoy_core::slow_gate::{CLAIM_KEY, LABEL, LANDINGS, PERCENT, TITLE};
use common::{BASE, World, commit, git};
use serde_json::{Value, json};

/// The gate command of every world, under its gate limit of 600 seconds.
const COMMAND: &str = "./gate.sh";

/// Lands one commit on the base branch whose gate run, finished at `at`, took
/// `seconds` on `host` and is on record → the commit. No host: the record
/// states no time.
fn land(world: &World, name: &str, host: &str, seconds: u64, at: u64) -> String {
    let landed = commit(&world.clone, name, name);
    git(&world.clone, &["push", "-q", "origin", BASE]);
    let repo = Repo::at(&world.clone).unwrap();
    let tree = Tree::named(&git(&world.clone, &["rev-parse", "HEAD^{tree}"]));
    let command = GateCommand::new(COMMAND);
    let ran =
        Ran { exit_code: 0, timed_out: false, clean_before: true, clean_after: true, seconds };
    let Recording::Write(run) = after_run(&tree, &command, ran, host, at) else {
        panic!("a green run on a committed tree is put on record")
    };
    let record = repo.record_commit_of(tree.name(), &run.write(&tree, &command)).unwrap();
    let to = format!("{record}:{}", record_ref(repo.layout(), &tree, &command));
    git(&world.clone, &["push", "-q", "origin", &to]);
    landed
}

/// Lands one commit for each of `seconds`, oldest first, on `host`, the first
/// finished at `from` → the commits, newest first.
fn land_all(world: &World, host: &str, seconds: &[u64], from: u64) -> Vec<String> {
    let each = seconds.iter().enumerate();
    let mut landed: Vec<String> = each
        .map(|(n, took)| land(world, &format!("{host}-{from}-{n}"), host, *took, from + n as u64))
        .collect();
    landed.reverse();
    landed
}

fn tick_as(world: &World, instance: &str, host: &str, now: &str) -> Value {
    let args = ["tick", "--instance", instance, "--host", host, "--now", now];
    world.afk(&[&args[..], &[]].concat()).ok()
}

/// What the tick's `slow_gate` step came to; `None`: it made none.
fn opening(ticked: &Value) -> Option<&str> {
    let did = ticked["did"].as_array().unwrap().iter();
    did.filter(|step| step["do"] == "slow_gate").find_map(|step| step["outcome"].as_str())
}

/// The issues GitHub has that carry the slow-gate label.
fn opened(world: &World) -> Vec<Value> {
    let issues = world.gh()["issues"].as_array().unwrap().clone();
    let carries = |issue: &Value| issue["labels"].as_array().unwrap().contains(&json!(LABEL));
    issues.into_iter().filter(carries).collect()
}

fn plan(world: &World, now: &str) -> Value {
    world.afk(&["plan", "--now", now]).ok()
}

#[test]
fn a_tick_opens_one_issue_for_a_slow_gate_and_no_other_while_it_is_open() {
    let world = World::new();
    let landed = land_all(&world, "mini", &[481, 590, 500, 599, 550], 90_000);

    // `plan` says so before anything is opened
    let slow = plan(&world, "91000")["slow_gate"].clone();
    assert_eq!(
        (&slow["gate_seconds"], &slow["host"], &slow["issue"]),
        (&json!(600), &json!("mini"), &json!(null))
    );
    let runs: Vec<(String, u64)> = slow["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| (run["commit"].as_str().unwrap().to_string(), run["seconds"].as_u64().unwrap()))
        .collect();
    let times = [550, 599, 500, 590, 481];
    let evidence: Vec<(String, u64)> = landed.iter().cloned().zip(times).collect();
    assert_eq!(runs, evidence, "the five, newest first");

    let ticked = tick_as(&world, "alpha", "h1", "91000");
    assert_eq!(opening(&ticked), Some("done"), "{ticked}");
    assert_eq!(ticked["errors"], json!([]));
    let issues = opened(&world);
    assert_eq!(issues.len(), 1, "{issues:?}");
    let issue = &issues[0];
    assert_eq!(issue["title"], TITLE);
    assert_eq!(issue["labels"], json!(["ready-for-agent", LABEL]), "the fleet works it");
    let body = issue["body"].as_str().unwrap();
    assert!(body.contains("On the host `mini`"), "{body}");
    assert!(body.contains("gate limit of 600s (`gate_seconds`)"), "{body}");
    for (commit, seconds) in &evidence {
        assert!(body.contains(&format!("- `{commit}` took {seconds}s\n")), "{body}");
    }
    assert!(body.contains("deleting or skipping tests, and raising `gate_seconds`"), "{body}");
    // the claim it was opened under is gone
    let claims = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claims"]);
    assert!(!claims.contains(CLAIM_KEY), "{claims}");

    // `plan` and `doctor` name it, with the same evidence
    let number = issue["number"].as_u64().unwrap();
    let named = plan(&world, "91100")["slow_gate"].clone();
    assert_eq!(named["issue"], number);
    assert_eq!(named["runs"], slow["runs"]);
    let findings =
        world.afk(&["doctor", "--host", "h1", "--now", "91100"]).json["findings"].clone();
    let finding = findings
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["what"].as_str().unwrap().contains("the gate is slow"))
        .unwrap_or_else(|| panic!("{findings}"));
    assert_eq!(finding["severity"], "warning");
    assert_eq!(finding["what"], format!("the gate is slow: #{number} is open for it"));
    assert_eq!(
        finding["fix"],
        format!("afkconvoy show -C '{}' --issue {number}", world.clone.display())
    );
    let said = finding["evidence"].as_str().unwrap();
    assert!(said.contains("on the host mini") && said.contains("gate limit of 600s"), "{said}");
    for (commit, seconds) in &evidence {
        assert!(said.contains(&format!("{commit} {seconds}s")), "{said}");
    }

    // while it is open no tick opens another, on any host
    assert_eq!(opening(&tick_as(&world, "alpha", "h1", "91200")), None);
    assert_eq!(opening(&tick_as(&world, "beta", "h2", "91300")), None);
    land(&world, "one-more", "mini", 590, 91_350);
    assert_eq!(opening(&tick_as(&world, "alpha", "h1", "91400")), None);
    assert_eq!(opened(&world).len(), 1);
}

#[test]
fn a_fixed_budget_warns_through_tick_and_doctor_without_mixing_hosts_or_reopening() {
    let world = World::new();
    world.set_gate_config("gate_command: ./gate.sh\ngate_seconds: 3600\ngate_budget_seconds: 600");
    // Controlled recorded command durations: no real ten-minute waits.
    // The observed six-minute gate and the exact budget are both normal.
    land_all(&world, "mini", &[379, 379, 600, 379, 379], 90_000);
    let normal = tick_as(&world, "alpha", "h1", "91000");
    assert_eq!(opening(&normal), None);
    assert_eq!(plan(&world, "91000").get("slow_gate"), None);
    let findings = world.afk(&["doctor", "--host", "h1", "--now", "91000"]).json;
    assert!(
        !findings["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| { finding["what"].as_str().unwrap().contains("the gate is slow") })
    );

    // Five slow runs split across two hosts cannot make either host slow.
    land_all(&world, "mini", &[601, 650, 700], 91_100);
    land_all(&world, "air", &[800, 900], 91_200);
    assert_eq!(opening(&tick_as(&world, "alpha", "h1", "91400")), None);
    assert_eq!(opened(&world).len(), 0);

    land_all(&world, "mini", &[750, 800], 91_500);
    let slow = plan(&world, "91600")["slow_gate"].clone();
    assert_eq!(slow["gate_seconds"], 3600, "termination limit is independent");
    assert_eq!(slow["gate_budget_seconds"], 600, "fixed, never learned from these runs");
    assert_eq!(slow["host"], "mini");
    let times: Vec<u64> = slow["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["seconds"].as_u64().unwrap())
        .collect();
    assert_eq!(times, [800, 750, 700, 650, 601]);
    // One real foreground tick also persists the existing tick log.
    let (lines, _) = world.afk_lines(&[
        "run",
        "--foreground",
        "--host",
        "h1",
        "--instance",
        "alpha",
        "--ticks",
        "1",
        "--now",
        "91600",
        "--second-ms",
        "0",
    ]);
    let line =
        lines.iter().find(|line| line.contains("opened an issue for the slow gate")).unwrap();
    assert!(line.contains("fixed duration budget of 600s"), "{line}");
    let logs = std::fs::read_dir(world.cache().join("afkconvoy/runs")).unwrap();
    let ticked = logs
        .flat_map(|file| {
            let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
            text.lines()
                .map(|line| serde_json::from_str::<afkconvoy::log::Entry>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .find_map(|entry| match entry.what {
            afkconvoy::log::What::Tick(tick) => tick.answer,
            _ => None,
        })
        .expect("the tick's answer is persisted in the existing log");
    assert_eq!(opening(&ticked), Some("done"));
    let logged =
        ticked["did"].as_array().unwrap().iter().find(|step| step["do"] == "slow_gate").unwrap();
    let logged = logged["evidence"].as_str().unwrap();
    assert!(logged.contains("fixed duration budget of 600s"), "{logged}");
    assert!(logged.contains("termination gate limit is 3600s"), "{logged}");
    let issues = opened(&world);
    assert_eq!(issues.len(), 1);
    let body = issues[0]["body"].as_str().unwrap();
    assert!(body.contains("fixed duration budget of 600s"), "{body}");
    assert!(body.contains("termination gate limit is 3600s"), "{body}");
    let findings = world.afk(&["doctor", "--host", "h1", "--now", "91700"]).json;
    let finding = findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["what"].as_str().unwrap().contains("the gate is slow"))
        .unwrap();
    assert_eq!(finding["severity"], "warning");
    let evidence = finding["evidence"].as_str().unwrap();
    assert!(evidence.contains("fixed duration budget of 600s"), "{evidence}");
    for run in slow["runs"].as_array().unwrap() {
        assert!(evidence.contains(run["commit"].as_str().unwrap()), "{evidence}");
    }
    // Another observing host and even five slow runs on another host reuse the issue.
    land_all(&world, "air", &[950, 960, 970], 91_800);
    assert_eq!(opening(&tick_as(&world, "beta", "h2", "92000")), None);
    assert_eq!(opened(&world).len(), 1);
}

#[test]
fn landings_that_do_not_say_the_gate_is_slow_open_nothing() {
    let quiet = |world: &World| {
        let ticked = tick_as(world, "alpha", "h1", "91000");
        assert_eq!(opening(&ticked), None, "{ticked}");
        assert_eq!(plan(world, "91000").get("slow_gate"), None);
        assert_eq!(opened(world), [] as [Value; 0]);
        // …and GitHub is not asked for the issues of a slow gate
        let asked = world.gh_calls().iter().any(|call| call.join(" ").contains("state=all"));
        assert!(!asked, "{:?}", world.gh_calls());
    };

    // one of the five at 80% of the limit, and no more
    let world = World::new();
    land_all(&world, "mini", &[590, 590, 480, 590, 590], 90_000);
    quiet(&world);

    // fewer than five with a time: the fifth's record states none
    let world = World::new();
    land_all(&world, "mini", &[590, 590, 590, 590], 90_000);
    land(&world, "no-time", "", 590, 90_100);
    quiet(&world);

    // five, spread over hosts none of which has five
    let world = World::new();
    land_all(&world, "mini", &[590, 590, 590], 90_000);
    land_all(&world, "air", &[590, 590], 90_100);
    quiet(&world);

    // five of another gate command's
    let world = World::new();
    land_all(&world, "mini", &[590; 5], 90_000);
    world.set_gate("make test");
    quiet(&world);
}

#[test]
fn after_the_issue_is_closed_only_the_landings_made_since_count() {
    let world = World::new();
    land_all(&world, "mini", &[590; 5], 90_000);
    assert_eq!(opening(&tick_as(&world, "alpha", "h1", "91000")), Some("done"));
    let first = opened(&world)[0]["number"].as_u64().unwrap();
    world.with_gh(|gh| {
        let issue = gh["issues"].as_array_mut().unwrap().iter_mut().find(|i| i["number"] == first);
        let issue = issue.unwrap();
        issue["state"] = json!("closed");
        issue["closed_at"] = json!(91_500);
    });

    // what landed before it closed is what it was about
    assert_eq!(opening(&tick_as(&world, "alpha", "h1", "91600")), None);
    assert_eq!(plan(&world, "91600").get("slow_gate"), None);
    // four since are not five, whatever is under them
    land_all(&world, "mini", &[590; 4], 91_700);
    assert_eq!(opening(&tick_as(&world, "alpha", "h1", "91800")), None);
    assert_eq!(opened(&world).len(), 1);

    let since = land_all(&world, "mini", &[590], 91_900);
    let ticked = tick_as(&world, "alpha", "h1", "92000");
    assert_eq!(opening(&ticked), Some("done"), "{ticked}");
    let issues = opened(&world);
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues[1]["body"].as_str().unwrap().contains(&since[0]), "the newest landing");
}

#[test]
fn of_two_hosts_that_find_the_gate_slow_one_opens_the_issue() {
    let world = World::new();
    land_all(&world, "mini", &[590; 5], 90_000);
    // a peer's tick is at the opening: it holds the claim
    let repo = Repo::at(&world.clone).unwrap();
    let peer = Me { instance: InstanceId::parse("beta").unwrap(), host: "h2".into(), now: 91_000 };
    let mut scan = Scan::read(&repo).unwrap();
    assert!(take_run(&repo, &mut scan, &peer, CLAIM_KEY).unwrap().is_some());

    let ticked = tick_as(&world, "alpha", "h1", "91000");
    assert_eq!(opening(&ticked), Some("lost"), "{ticked}");
    assert_eq!(opened(&world), [] as [Value; 0], "the peer's to open");

    // a peer that never did is presumed dead once its lease ran out: its claim opens nothing
    let ticked = tick_as(&world, "alpha", "h1", "99000");
    assert_eq!(opening(&ticked), Some("done"), "{ticked}");
    assert_eq!(opened(&world).len(), 1);
}

#[test]
fn the_help_says_the_rule_as_the_core_has_it() {
    let rule = format!("the last {LANDINGS} landings");
    let most = format!("more than {PERCENT}%");
    for topic in [&["tick", "--help"][..], &["plan", "--help"], &["help", "troubleshooting"]] {
        let out = world_free(topic);
        let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(text.contains(&rule) && text.contains(&most), "{topic:?} says the rule");
    }
}

/// What the binary prints for `args`, outside any repo.
fn world_free(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(args).output();
    String::from_utf8(out.unwrap().stdout).unwrap()
}

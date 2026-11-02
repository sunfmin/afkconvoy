use crate::common::World;
use afkconvoy_core::account::{self, Begun, Heard, Machine, Told};
use serde_json::{Value, json};

/// Use the actual writer: this report must keep reading what the fleet leaves.
fn account(number: u64, start: u64, end: u64, harnesses: &[&str], gates: &[&str]) -> String {
    let starts: Vec<String> = harnesses
        .iter()
        .map(|harness| {
            account::worker_comment(&Begun {
                at: start,
                since: Some(start),
                claim: "claim",
                harness,
                model: None,
                stage: None,
                role: None,
                consult: false,
            })
        })
        .collect();
    let heard: Vec<Heard<'_>> = starts.iter().map(|body| Heard { at: Some(start), body }).collect();
    account::comment(&Told {
        issue: number,
        repo: "acme/dev",
        tracker: "acme/tracker",
        version: "test",
        outcome: account::End::Landed,
        lanes: &[("./gate.sh", "whole")],
        heard: &heard,
        gates,
        claimed: true,
        held_since: Some(start),
        end,
        machine: Machine { chip: None, model: None, cpus: None, bytes: None },
        host: "test",
    })
    .unwrap()
}

#[test]
fn accounts_match_a_hand_count_and_only_read_the_trackers_repos_issues() {
    let world = World::new();
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, "acme/tracker");
    world.with_tracker(|gh| {
        gh["issues"] = json!((1..=8).map(|number| json!({"number": number, "title": "issue",
            "state": if number == 1 {"open"} else {"closed"},
            "labels": match number {
                7 => vec!["repo:acme/other"],
                8 => vec!["repo:acme/dev", "repo:acme/other"],
                _ => vec!["repo:acme/dev"],
            }})).collect::<Vec<_>>());
        for (number, wall, harnesses) in [
            (1, 10, vec!["codex", "claude-code"]), (2, 20, vec!["codex"]),
            (3, 30, vec!["claude-code"]), (4, 40, vec!["claude-code"]),
            (7, 900, vec!["codex"]), (8, 900, vec!["codex"]),
        ] {
            let gates = [format!("convoy:gate tree=abc command=check issue={number} seconds={number} at=900"),
                format!("convoy:gate tree=def command=check issue={number} seconds={number} at=901 red=1")];
            let body = account(number, 1000 - wall, 1000, &harnesses,
                &gates.iter().map(String::as_str).collect::<Vec<_>>());
            gh["comments"][number.to_string()] = json!([{"id": number, "body": body}]);
        }
        gh["comments"]["5"] = json!([{"id": 5, "body": "<!--convoy:account start=unknown end=1000-->"}]);
        // Account past the embedded comment page. No account is silently missed.
        let mut comments: Vec<_> = (0..101).map(|id| json!({"id": id, "body": "discussion"})).collect();
        comments.push(json!({"id": 999, "body": "no account yet"}));
        gh["comments"]["6"] = json!(comments);
        for rows in gh["comments"].as_object_mut().unwrap().values_mut() {
            for row in rows.as_array_mut().unwrap() {
                row["html_url"] = json!("https://gh/comment");
            }
        }
    });
    let before = world.tracker();
    let out = world.afk(&["accounts", "--all", "--now", "2000"]).ok();
    assert_eq!(out["window"], json!({"start": null, "end": 2000}));
    assert_eq!(out["count"], 4);
    assert_eq!(out["wall_seconds"], json!({"p50": 20, "p90": 40, "p95": 40, "max": 40}));
    assert_eq!(
        out["by_harness"]["codex"],
        json!({"count": 2,
        "wall_seconds": {"p50": 10, "p90": 20, "p95": 20, "max": 20}})
    );
    assert_eq!(out["multiple_workers"], 1);
    assert_eq!(out["gate_seconds"], json!({"p50": 4, "p95": 8}));
    assert_eq!(out["unreadable"][0]["number"], 5);
    assert_eq!(out["unreadable"].as_array().unwrap().len(), 1);
    assert_eq!(
        out["slowest"],
        json!([
        {"number": 4, "url": "https://gh/issues/4", "wall_seconds": 40},
        {"number": 3, "url": "https://gh/issues/3", "wall_seconds": 30},
        {"number": 2, "url": "https://gh/issues/2", "wall_seconds": 20},
        {"number": 1, "url": "https://gh/issues/1", "wall_seconds": 10}])
    );
    let after = world.tracker();
    for key in ["issues", "comments", "labels"] {
        assert_eq!(before[key], after[key]);
    }
    assert!(world.orca_calls().is_empty());
}

#[test]
fn account_windows_select_the_end_and_report_unknown_data() {
    let world = World::new();
    for (number, end) in [(1, 10), (2, 86410), (3, 86411), (4, 86412)] {
        world.issue(number, &[]);
        world.comment(number, &account(number, 0, end, &[], &[]));
    }
    world.issue(5, &[]);
    world.comment(5, "<!--convoy:account start=0 end=broken-->");
    world.issue(6, &[]);
    world.comment(
        6,
        &account(6, 0, 86410, &["codex"], &["convoy:gate tree=abc command=check issue=6"]),
    );
    let out = world.afk(&["accounts", "--days", "1", "--now", "86411"]).ok();
    assert_eq!(out["count"], 2);
    assert_eq!(out["wall_seconds"]["p50"], 86410);
    assert_eq!(out["gate_seconds"], json!({"p50": 0, "p95": 0}));
    assert_eq!(out["by_harness"]["unknown"]["count"], 2);
    assert_eq!(out["unreadable"].as_array().unwrap().len(), 2);
    assert_eq!(out["unreadable"][0]["number"], 5);
    assert_eq!(out["unreadable"][1]["number"], 6);
    let empty = world.afk(&["accounts", "--now", "0"]).ok();
    assert_eq!(empty["count"], 0);
    assert_eq!(empty["window"]["start"], 0);
    assert_eq!(empty["wall_seconds"], json!({"p50": null, "p90": null, "p95": null, "max": null}));
    assert_eq!(empty["slowest"], json!([]));
    let asked = |args: &[&str]| {
        let out = world
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let help = asked(&["accounts", "--help"]);
    for text in [
        "nearest rank",
        "writes nothing",
        "tracker",
        "watch_rounds",
        "unknown_rounds",
        "watch_round",
        "asking_question",
        "patrol_run",
        "standby excluded",
    ] {
        assert!(help.contains(text), "{help}");
    }
    assert!(asked(&["--help"]).contains("accounts"));
    assert!(world.afk(&["accounts", "--days", "0"]).error().contains("invalid value"));
    assert!(world.afk(&["accounts", "--days", "1", "--all"]).error().contains("cannot be used"));
}

#[test]
fn rows_are_ndjson_and_prose_changes_do_not_change_either_answer() {
    let world = World::new();
    let first = account(
        1,
        100,
        200,
        &["codex"],
        &["convoy:gate tree=abc command=./gate.sh issue=1 seconds=12 at=150 host=test"],
    );
    world.issue(1, &[]);
    world.comment(1, &first);
    world.issue(2, &[]);
    world.comment(2, &account(2, 100, 300, &[], &[]));
    let rows = || {
        let out = world
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
            .args(["accounts", "--all", "--rows", "--now", "250"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        let rows: Vec<serde_json::Value> =
            text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(rows.len(), 1, "one line per account in the window");
        rows
    };
    let before_rows = rows();
    let row = &before_rows[0];
    assert_eq!(row["schema_version"], 1);
    assert_eq!(row["issue"], 1);
    assert_eq!(row["url"], "https://gh/issues/1");
    assert_eq!(row["repo"], "acme/dev");
    assert_eq!(row["tracker"], "acme/tracker");
    assert_eq!(row["afkconvoy_version"], "test");
    assert_eq!(row["outcome"], "landed");
    assert_eq!(row["workers"][0]["seconds"], 100);
    assert_eq!(row["workers"][0]["model"], serde_json::Value::Null);
    assert_eq!(row["gates"][0]["lane"], "whole");
    let before = world.afk(&["accounts", "--all", "--now", "250"]).ok();
    world.with_gh(|gh| {
        // Replace the whole human rendering, including headings and durations.
        let (_, data) = first.split_once("<details>").unwrap();
        gh["comments"]["1"][0]["body"] =
            json!(format!("<!--convoy:account -->\nNew words.\n<details>{data}"));
    });
    assert_eq!(rows(), before_rows);
    assert_eq!(world.afk(&["accounts", "--all", "--now", "250"]).ok(), before);
}

#[test]
fn rewrite_previews_checks_and_is_idempotent() {
    let world = World::new();
    world.issue(421, &[]);
    let body = include_str!("fixtures/account-421.md");
    world.comment(421, body);
    world.issue(422, &[]);
    let unexpected = body.replace("Machine: Apple", "Other fact: keep me\nMachine: Apple");
    world.comment(422, &unexpected);
    world.issue(423, &[]);
    let damaged = "<!--convoy:account -->\n```json\n{}\n```";
    world.comment(423, damaged);
    let before = world.gh();
    let dry = world.afk(&["rewrite-accounts"]).ok();
    assert_eq!(dry["rewrite"], 1);
    assert_eq!(dry["left"], 2);
    assert_eq!(dry["changes"][0]["old_body"], body);
    assert_eq!(dry["changes"][0]["check"], "pass");
    assert_eq!(world.gh()["comments"], before["comments"]);
    let backup = world.clone.join("account-backups");
    let path = backup.to_str().unwrap();
    let written = world.afk(&["rewrite-accounts", "--backup", path]).ok();
    assert_eq!(written["rewrite"], 1);
    assert_eq!(world.gh()["comments"]["422"][0]["body"], unexpected);
    assert_eq!(world.gh()["comments"]["423"][0]["body"], damaged);
    assert_eq!(std::fs::read_dir(&backup).unwrap().count(), 1);
    let again = world.afk(&["rewrite-accounts", "--backup", path]).ok();
    assert_eq!(again["rewrite"], 0);
    assert_eq!(again["left"], 2);
    let report = world.afk(&["accounts", "--all", "--now", "1793542000"]).ok();
    assert_eq!(report["count"], 1);
    assert_eq!(report["wall_seconds"]["p50"], 2339);
    assert_eq!(report["gate_seconds"]["p50"], 966);
}

#[test]
fn rewrite_uses_an_adjacent_escalation_as_the_outcome() {
    let world = World::new();
    world.issue(421, &[]);
    world.comment(421, "<!--convoy:escalation claim=test-->\nEscalated.");
    world.comment(421, include_str!("fixtures/account-421.md"));
    let dry = world.afk(&["rewrite-accounts"]).ok();
    let body = dry["changes"][0]["new_body"].as_str().unwrap();
    let row = account::read(body, 421).unwrap();
    assert_eq!(row.outcome.as_deref(), Some("escalated"));
}

#[test]
fn unknown_schema_is_unreadable_and_unknown_measurements_stay_null_in_rows() {
    let world = World::new();
    world.issue(1, &[]);
    world.comment(
        1,
        &account(1, 100, 200, &[], &[]).replace("\"schema_version\": 1", "\"schema_version\": 999"),
    );
    world.issue(2, &[]);
    world.comment(2, &account(2, 100, 200, &[], &["convoy:gate tree=abc command=check issue=2"]));
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["accounts", "--all", "--rows", "--now", "250"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let row: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(row["issue"], 2);
    assert!(row["gates"][0]["seconds"].is_null());
    let diagnostics: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(diagnostics["unreadable"][0]["number"], 1);
    assert!(
        diagnostics["unreadable"][0]["reason"].as_str().unwrap().contains("schema version: 999")
    );
    let summary = world.afk(&["accounts", "--all", "--now", "250"]).ok();
    assert_eq!(summary["count"], 0);
    assert_eq!(summary["unreadable"].as_array().unwrap().len(), 2);
}

#[test]
fn token_summary_counts_known_workers_by_harness_and_ranks_issues() {
    use afkconvoy_core::account::Counts;
    let world = World::new();
    for n in 1..=6u64 {
        world.issue(n, &[]);
        let text =
            account(n, 100, 200, &[if n % 2 == 0 { "codex" } else { "pi" }, "claude-code"], &[]);
        let mut row = account::read(&text, n).unwrap();
        row.workers[0].tokens = Some(
            [(
                String::from("model"),
                Counts {
                    input: n * 10,
                    cache_read: n * 2,
                    cache_write: n,
                    output: n * 3,
                    reasoning: Some(n),
                },
            )]
            .into_iter()
            .collect(),
        );
        world.comment(n, &account::render(&row));
    }
    let out = world.afk(&["accounts", "--all", "--now", "300"]).ok();
    assert_eq!(
        out["tokens"]["totals"],
        json!({"input":210,"cache_read":42,"cache_write":21,"output":63,"reasoning":21})
    );
    assert_eq!(out["tokens"]["unknown_workers"], 6);
    assert_eq!(out["tokens"]["p50"], 48);
    assert_eq!(out["tokens"]["p95"], 96);
    assert_eq!(out["tokens"]["by_harness"]["codex"]["totals"]["input"], 120);
    assert_eq!(out["tokens"]["by_harness"]["pi"]["totals"]["input"], 90);
    assert_eq!(out["tokens"]["by_harness"]["claude-code"]["unknown_workers"], 6);
    assert_eq!(
        out["tokens"]["top"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![6, 5, 4, 3, 2]
    );
    let rows = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["accounts", "--all", "--rows", "--now", "300"])
        .output()
        .unwrap();
    assert!(rows.status.success());
    let row: serde_json::Value =
        serde_json::from_slice(rows.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(row["workers"][0]["tokens"]["model"]["input"], 10);
    assert!(row["workers"][1]["tokens"].is_null());
}

#[test]
fn watch_rounds_match_a_hand_count_and_answer_their_own_ndjson_rows() {
    use afkconvoy_core::account::Counts;
    use afkconvoy_core::accounts::Usage;
    let world = World::new();
    world.issue(1, &[]);
    for n in 1..=7u64 {
        let tokens = (n != 7).then(|| {
            [(
                String::from("model"),
                Counts {
                    input: n * 10,
                    cache_read: n * 2,
                    cache_write: n,
                    output: n * 3,
                    reasoning: Some(n),
                },
            )]
            .into_iter()
            .collect()
        });
        let usage = Usage {
            id: format!("round{n}"),
            harness: Some(if n % 2 == 0 { "codex" } else { "pi" }.into()),
            model: Some("model".into()),
            start: Some(50),
            end: Some(100),
            seconds: Some(50),
            tokens,
            conditions: vec![format!("condition{n}")],
        };
        let dir = world.cache().join("afkconvoy/watch").join(&usage.id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("usage.json"), serde_json::to_vec(&usage).unwrap()).unwrap();
    }
    let out = world.afk(&["accounts", "--days", "1", "--now", "86411"]).ok();
    let rounds = &out["watch_rounds"];
    assert_eq!(rounds["count"], 7);
    assert_eq!(rounds["unknown_rounds"], 1);
    assert_eq!(
        rounds["totals"],
        json!({"input":210,"cache_read":42,"cache_write":21,"output":63,"reasoning":21})
    );
    assert_eq!(rounds["p50"], 48);
    assert_eq!(rounds["p95"], 96);
    assert_eq!(rounds["by_harness"]["codex"]["count"], 3);
    assert_eq!(rounds["by_harness"]["codex"]["totals"]["input"], 120);
    assert_eq!(rounds["by_harness"]["pi"]["count"], 4);
    assert_eq!(rounds["by_harness"]["pi"]["unknown_rounds"], 1);
    assert_eq!(rounds["by_harness"]["pi"]["totals"]["input"], 90);
    assert_eq!(
        rounds["top"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["round6", "round5", "round4", "round3", "round2"]
    );
    assert_eq!(rounds["top"][0]["conditions"], json!(["condition6"]));
    assert_eq!(rounds["top"][0]["total"], 96);

    let rows = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["accounts", "--days", "1", "--rows", "--now", "86411"])
        .output()
        .unwrap();
    assert!(rows.status.success(), "{rows:?}");
    let rows: Vec<Value> = String::from_utf8(rows.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 7);
    for (n, row) in (1..=7).zip(rows) {
        assert_eq!(row["kind"], "watch_round");
        assert_eq!(row["id"], format!("round{n}"));
        assert_eq!(row["conditions"], json!([format!("condition{n}")]));
        assert_eq!(row["seconds"], 50);
        if n == 7 {
            assert!(row["tokens"].is_null());
        } else {
            assert_eq!(row["tokens"]["model"]["input"], n * 10);
        }
    }
}

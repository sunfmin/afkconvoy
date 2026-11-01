use crate::common::World;
use afkconvoy_core::account::{self, Begun, Heard, Machine, Told};
use serde_json::json;

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
                consult: false,
            })
        })
        .collect();
    let heard: Vec<Heard<'_>> = starts.iter().map(|body| Heard { at: Some(start), body }).collect();
    account::comment(&Told {
        issue: number,
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
    for text in ["nearest rank", "writes nothing", "tracker"] {
        assert!(help.contains(text), "{help}");
    }
    assert!(asked(&["--help"]).contains("accounts"));
    assert!(world.afk(&["accounts", "--days", "0"]).error().contains("invalid value"));
    assert!(world.afk(&["accounts", "--days", "1", "--all"]).error().contains("cannot be used"));
}

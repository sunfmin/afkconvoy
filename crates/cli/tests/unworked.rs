//! The actual doctor/runner path, with GitHub timing records and a repo
//! absent from the checkout list. Its heartbeat refs are real git refs.

use crate::common::{World, git};
use afkconvoy_core::claims::{InstanceId, write_heartbeat};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn setup() -> (World, PathBuf, u64) {
    let world = World::new();
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, "acme/control");
    let remote = world.repo(
        "acme/other",
        &[("docs/agents/afkconvoy.md", "```yaml\nready_label: take-me\n```\n")],
    );
    git(
        &world.clone,
        &[
            "config",
            &format!("url.{}.insteadOf", remote.display()),
            "https://github.com/acme/other.git",
        ],
    );
    git(
        &world.clone,
        &[
            "config",
            "--add",
            &format!("url.{}.insteadOf", world.bare.display()),
            "https://github.com/acme/control.git",
        ],
    );
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    world.with_tracker(|gh| {
        gh["bare"] = json!(world.bare);
        gh["issues"] = json!([{"number": 7, "title": "work", "body": "", "state": "open",
            "labels": ["repo:acme/other", "take-me"], "at": now - 10_000}]);
        gh["labels"] = json!(["repo:acme/other", "take-me", "ready-for-human"]);
    });
    ready(&world, now - 3660);
    (world, remote, now)
}

fn ready(world: &World, at: u64) {
    world.with_tracker(|gh| gh["timelines"]["7"] = json!([
        {"event": "labeled", "label": {"name": "take-me"}, "created_at": afkconvoy::log::stamp(at)}
    ]));
}

fn observation(world: &World) -> Value {
    let answer = world.afk(&["doctor", "--all"]).json;
    let rows = answer["host"]["unworked_repos"].as_array().unwrap_or_else(|| panic!("{answer}"));
    rows.iter().find(|r| r["repo"] == "acme/other").unwrap().clone()
}

fn record(remote: &std::path::Path, name: &str, message: &str) {
    let tree = git(remote, &["hash-object", "-t", "tree", "/dev/null"]);
    let sha = git(remote, &["commit-tree", tree.trim(), "-m", message]);
    git(remote, &["update-ref", name, sha.trim()]);
}

#[test]
fn doctor_reads_label_and_blocker_times_and_remote_beats_without_a_checkout() {
    let (world, remote, now) = setup();
    ready(&world, now - 3540);
    let young = observation(&world);
    assert!(young["waiting"].as_array().unwrap().is_empty(), "{young}");
    assert!(young["error"].is_null(), "{young}");
    ready(&world, now - 3660);
    let old = observation(&world);
    assert_eq!(old["waiting"][0]["issue"], 7, "{old}");
    assert!(old["newest"].is_null());
    assert!(old["error"].is_null(), "{old}");
    assert_eq!(
        world.tracker()["issues"].as_array().unwrap().len(),
        1,
        "doctor never opens a report"
    );
    world.with_tracker(|gh| gh["edges"]["7"] = json!([{"number": 9, "state": "open"}]));
    assert!(observation(&world)["waiting"].as_array().unwrap().is_empty());
    world.with_tracker(|gh| {
        gh["edges"]["7"] = json!([{"number": 9, "state": "closed", "closed_at": now - 3540}])
    });
    assert!(observation(&world)["waiting"].as_array().unwrap().is_empty());
    world.with_tracker(|gh| gh["edges"]["7"][0]["closed_at"] = json!(now - 3660));
    assert_eq!(observation(&world)["waiting"][0]["issue"], 7);
    record(
        &remote,
        "refs/heads/convoy-heartbeat/peer",
        &write_heartbeat(&InstanceId::parse("peer").unwrap(), "taking-host", now - 1740),
    );
    let live = observation(&world);
    assert_eq!(live["newest"]["host"], "taking-host", "{live}");
    assert_eq!(live["recovered"], "taking-host", "{live}");
    git(&remote, &["update-ref", "-d", "refs/heads/convoy-heartbeat/peer"]);
    // Even a malformed claim excludes a waiting issue: takeover owns it.
    record(&remote, "refs/convoy/claim/acme/control/7", "unreadable claim owner");
    assert!(observation(&world)["waiting"].as_array().unwrap().is_empty());
}

fn until(world: &World, check: impl Fn() -> Option<Value>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "tracker: {}; runner: {:?}",
            world.tracker(),
            world.runner_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn runner_opens_one_report_starts_its_watch_and_closes_on_a_live_heartbeat() {
    let (world, remote, now) = setup();
    world.afk(&["run", "--host", "h1", "--second-ms", "100", "--watch-every", "1"]).ok();
    let report = until(&world, || {
        world.tracker()["issues"]
            .as_array()?
            .iter()
            .find(|i| {
                i["body"].as_str().is_some_and(|body| body.starts_with("<!--convoy:unworked "))
            })
            .cloned()
    });
    assert!(report["labels"].as_array().unwrap().contains(&json!("repo:acme/other")), "{report}");
    assert!(report["labels"].as_array().unwrap().contains(&json!("ready-for-human")), "{report}");
    let body = report["body"].as_str().unwrap();
    assert!(body.contains("acme/control#7") && body.contains("no heartbeat"), "{body}");
    let state_file = world.cache().join("afkconvoy/watch/state.json");
    let watch = until(&world, || {
        let bytes = std::fs::read(&state_file).ok()?;
        let state: Value = serde_json::from_slice(&bytes).ok()?;
        state["running"].as_object()?;
        state["running"]["items"]
            .as_array()?
            .iter()
            .any(|i| i["id"].as_str().is_some_and(|id| id.contains("unworked-repo")))
            .then_some(state["running"].clone())
    });
    let brief = std::fs::read_to_string(
        world.cache().join(format!("afkconvoy/watch/{}/brief-1.md", watch["id"].as_str().unwrap())),
    )
    .unwrap();
    // The actual round retains the repo evidence and the list is unchanged.
    assert_eq!(watch["look"]["host"]["unworked_repos"][0]["repo"], "acme/other");
    assert!(brief.contains("never add that repo to this host's checkout list"));
    assert_eq!(world.checkout_list()["checkouts"].as_array().unwrap().len(), 1);
    record(
        &remote,
        "refs/convoy/heartbeat/peer",
        &write_heartbeat(&InstanceId::parse("peer").unwrap(), "taking-host", now),
    );
    until(&world, || {
        world.tracker()["issues"]
            .as_array()?
            .iter()
            .find(|i| i["number"] == report["number"] && i["state"] == "closed")
            .cloned()
    });
    let tracker = world.tracker();
    assert_eq!(tracker["issues"].as_array().unwrap().len(), 2, "one report across later looks");
    assert!(
        tracker["comments"][report["number"].to_string()].to_string().contains("taking-host"),
        "{tracker}"
    );
}

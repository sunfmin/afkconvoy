//! `stop --release`: an operator's stop that gives back the claims its fleet
//! instance has at work, ends those workers, and only then takes the checkout
//! off the list. Plain `stop` releases nothing (docs/adr/0003).

use crate::common;

use std::path::{Path, PathBuf};

use common::{World, git};
use serde_json::{Value, json};

const TRACKER: &str = "acme/tracker";
const MINE: &str = "repo:acme/dev";
const CLAIM_7: &str = "refs/convoy/claim/acme/tracker/7";
const CLAIM_8: &str = "refs/convoy/claim/acme/tracker/8";
/// Another host's claim, on the same tracker.
const CLAIM_9: &str = "refs/convoy/claim/acme/tracker/9";

/// The checkout of `acme/dev`, on the list with `acme/tracker` its tracker,
/// whose fleet instance `alpha` has workers on the tracker's #7 and #8, and
/// another host's instance `beta` a claim on #9.
fn at_work() -> World {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, TRACKER);
    world.list_checkout(&world.clone);
    for number in [7, 8, 9] {
        world.with_tracker(|gh| {
            let issue = json!({"number": number, "title": format!("tracked {number}"),
                "labels": ["ready-for-agent", MINE], "state": "open"});
            gh["issues"].as_array_mut().unwrap().push(issue);
        });
    }
    let claim9 = ["claim", "--issue", "9", "--instance", "beta", "--host", "h2", "--now", "990"];
    assert_eq!(world.afk(&claim9).ok()["won"], true);
    let tick = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    world.afk(&tick).ok();
    assert_eq!(claims(&world), [CLAIM_7, CLAIM_8, CLAIM_9]);
    world
}

/// Every claim ref on the remote.
fn claims(world: &World) -> Vec<String> {
    let listed = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claim/"]);
    listed.lines().map(str::to_string).collect()
}

/// The worktree of the tracker's issue `number`.
fn worktree_of(world: &World, number: u64) -> PathBuf {
    let named =
        |path: &PathBuf| path.to_str().unwrap().contains(&format!("acme-tracker-{number}-"));
    world.worktrees().into_iter().rfind(named).unwrap()
}

fn listed(world: &World) -> Vec<Value> {
    world.checkout_list()["checkouts"].as_array().cloned().unwrap_or_default()
}

fn path(dir: &Path) -> &str {
    dir.to_str().unwrap()
}

#[test]
fn a_release_gives_back_the_claims_ends_the_workers_and_takes_the_checkout_off_the_list() {
    let world = at_work();
    let (seven, eight) = (worktree_of(&world, 7), worktree_of(&world, 8));
    std::fs::write(eight.join("unsaved.txt"), "work\n").unwrap();
    let labels = world.tracker()["issues"].clone();
    let beta = world.remote(&["rev-parse", CLAIM_9]);

    let stop = ["stop", "--release", "--instance", "alpha", "--now", "1100"];
    let answer = world.afk(&stop).ok();
    assert_eq!(answer["removed"], true, "{answer}");
    let released = answer["released"].as_array().unwrap();
    let refs: Vec<&str> = released.iter().map(|r| r["claim"].as_str().unwrap()).collect();
    assert_eq!(refs, [CLAIM_7, CLAIM_8], "{answer}");
    assert_eq!(released[0]["worktree_removed"], true, "{answer}");
    assert_eq!(released[1]["worktree_removed"], false, "{answer}");
    assert_eq!(released[1]["worktree"], path(&eight), "the dirty worktree is named: {answer}");
    assert!(released[1]["worktree_kept"].as_str().unwrap().contains("uncommitted"), "{answer}");

    // only alpha's claims are gone; beta's is as it was
    assert_eq!(claims(&world), [CLAIM_9]);
    assert_eq!(world.remote(&["rev-parse", CLAIM_9]), beta);
    // its workers are ended: every terminal closed, the clean worktree gone
    assert_eq!(world.open_terminals(), [] as [String; 0]);
    assert!(!seven.exists(), "{}", seven.display());
    assert!(eight.join("unsaved.txt").exists(), "a dirty worktree is kept");
    // off the list, and its heartbeat with it
    assert_eq!(listed(&world), [] as [Value; 0]);
    assert_eq!(answer["heartbeat_removed"], true, "{answer}");
    let beats = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/heartbeat/"]);
    assert!(!beats.contains("alpha"), "{beats}");
    // the issues keep their labels and stay open
    assert_eq!(world.tracker()["issues"], labels);

    // off the list the tracker is not said: neither command reads it as the
    // checkout's own repo's
    let scan = world.afk(&["scan"]).error();
    assert!(scan.contains("the tracker is not said"), "{scan}");
    let release = ["release", "--issue", "9", "--instance", "beta"];
    let error = world.afk(&release).error();
    assert!(error.contains("the tracker is not said"), "{error}");
    assert_eq!(claims(&world), [CLAIM_9]);
}

#[test]
fn a_plain_stop_releases_nothing_and_stops_no_worker() {
    let world = at_work();
    let answer = world.afk(&["stop"]).ok();
    assert_eq!(answer["removed"], true, "{answer}");
    assert_eq!(claims(&world), [CLAIM_7, CLAIM_8, CLAIM_9]);
    assert_eq!(world.open_terminals().len(), 2);
    assert_eq!(listed(&world), [] as [Value; 0]);
    let scan = world.afk(&["scan"]).error();
    assert!(scan.contains("the tracker is not said"), "{scan}");
}

#[test]
fn a_release_is_refused_with_all_with_a_cut_and_while_a_verdict_is_declared() {
    let world = at_work();
    let refused = world.afk(&["stop", "--all", "--release"]);
    assert!(!refused.json["error"].is_null() || refused.code != 0, "{}", refused.json);
    let stop = ["stop", "--release", "--instance", "alpha", "--now", "1100"];

    // a verdict declared under #7's claim: nothing is released
    world.with_tracker(|gh| {
        gh["comments"]["7"] = json!([{"id": 3001, "at": 1050, "html_url": "https://gh/c/3001",
            "body": "<!--convoy:verdict phase=escalate-->\nstuck"}]);
    });
    let answer = world.afk(&stop).ok();
    assert_eq!(answer["removed"], false, "{answer}");
    assert_eq!(answer["kept"][0]["claim"], CLAIM_7, "{answer}");
    assert_eq!(answer["released"], json!([]), "{answer}");
    assert_eq!(claims(&world), [CLAIM_7, CLAIM_8, CLAIM_9]);
    assert_eq!(listed(&world).len(), 1);
    assert_eq!(world.open_terminals().len(), 2);
    world.with_tracker(|gh| gh["comments"]["7"] = json!([]));

    // a cut held by the instance: refused, and nothing changes
    let record =
        afkconvoy_core::claims::Owner { instance: "alpha".into(), host: "h1".into(), ts: 1050 };
    let repo = afkconvoy::git::Repo::at(&world.clone).unwrap();
    let cut = repo.record_commit(&record.write()).unwrap();
    git(&world.clone, &["push", "-q", "origin", &format!("{cut}:refs/convoy/claim/cut")]);
    let error = world.afk(&stop).error();
    assert!(error.contains("holds the cut"), "{error}");
    assert_eq!(claims(&world), [CLAIM_7, CLAIM_8, CLAIM_9, "refs/convoy/claim/cut"]);
    assert_eq!(listed(&world).len(), 1);
}

#[test]
fn a_claim_that_moved_since_it_was_read_is_named_and_none_is_released() {
    let world = at_work();
    let repo = afkconvoy::git::Repo::at(&world.clone).unwrap();
    let seven = world.remote(&["rev-parse", CLAIM_7]);
    let eight = world.remote(&["rev-parse", CLAIM_8]);
    // a peer re-stamps #8 after it was read
    let theirs = repo.record_commit("convoy:claim instance=gamma host=h3 ts=1001").unwrap();
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{theirs}:{CLAIM_8}")]);

    let leases = [(CLAIM_7.to_string(), seven.clone()), (CLAIM_8.to_string(), eight)];
    let moved = afkconvoy::stop_release::unput_all(&repo, &leases).unwrap();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0]["claim"], CLAIM_8);
    assert!(moved[0]["why"].as_str().unwrap().contains("moved since it was read"));
    assert_eq!(world.remote(&["rev-parse", CLAIM_7]), seven, "none was released");
    assert_eq!(world.remote(&["rev-parse", CLAIM_8]), theirs);
}

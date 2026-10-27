use crate::common;

use common::{World, git};
use serde_json::json;

const CLAIM_7: &str = "refs/convoy/claim/7";

fn claim(world: &World, issue: &str, instance: &str, now: &str) -> serde_json::Value {
    world
        .afk(&["claim", "--issue", issue, "--instance", instance, "--host", "h1", "--now", now])
        .ok()
}

#[test]
fn the_first_claim_wins_and_the_loser_is_told_the_owner() {
    let world = World::new();
    let won = claim(&world, "7", "alpha", "1000");
    assert_eq!(won["won"], true);
    assert_eq!(won["ref"], CLAIM_7);
    assert_eq!(world.remote(&["rev-parse", CLAIM_7]), won["sha"].as_str().unwrap());

    let lost = claim(&world, "7", "beta", "1001");
    assert_eq!(lost["won"], false);
    assert_eq!(lost["owner"]["instance"], "alpha");
    assert_eq!(lost["owner"]["host"], "h1");
    assert_eq!(world.remote(&["rev-parse", CLAIM_7]), won["sha"].as_str().unwrap(), "untouched");
}

#[test]
fn of_instances_claiming_at_once_exactly_one_wins() {
    let world = World::new();
    let answers: Vec<_> = std::thread::scope(|scope| {
        let racers: Vec<_> = (0..6)
            .map(|i| {
                let world = &world;
                scope.spawn(move || claim(world, "7", &format!("fleet-{i}"), "1000"))
            })
            .collect();
        racers.into_iter().map(|racer| racer.join().unwrap()).collect()
    });
    let winners: Vec<_> = answers.iter().filter(|a| a["won"] == true).collect();
    assert_eq!(winners.len(), 1, "{answers:?}");
    for lost in answers.iter().filter(|a| a["won"] == false) {
        assert_eq!(lost["owner"]["instance"], winners[0]["instance"]);
    }
}

#[test]
fn a_push_that_failed_for_another_reason_is_not_a_lost_race() {
    let world = World::new();
    world.watch_pushes(Some("convoy/claim"));
    let error =
        world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "1000"]).error();
    assert!(error.contains("not a lost race"), "{error}");
    assert!(error.contains("rejected by the test"), "{error}");
}

#[test]
fn a_claim_is_never_on_the_remote_ahead_of_its_owners_heartbeat() {
    let world = World::new();
    world.watch_pushes(None);
    claim(&world, "7", "alpha", "1000");
    assert_eq!(world.pushes(), ["refs/convoy/target/7", "refs/convoy/heartbeat/alpha", CLAIM_7]);
    assert_eq!(world.afk(&["scan"]).ok()["heartbeats"], json!({"alpha": 1000}));

    // not due: a third of the lease has not passed
    claim(&world, "8", "alpha", "2500");
    assert_eq!(world.pushes().len(), 5, "{:?}", world.pushes());
    // due again
    claim(&world, "9", "alpha", "2501");
    assert_eq!(
        world.pushes()[5..],
        ["refs/convoy/target/9", "refs/convoy/heartbeat/alpha", "refs/convoy/claim/9"]
    );
    assert_eq!(world.afk(&["scan"]).ok()["heartbeats"], json!({"alpha": 2501}));
}

#[test]
fn heartbeat_refreshes_only_when_due() {
    let world = World::new();
    let beat = |now: &str| world.afk(&["heartbeat", "--instance", "alpha", "--now", now]).ok();
    assert_eq!(beat("1000")["refreshed"], true);
    assert_eq!(beat("2000")["refreshed"], false);
    assert_eq!(beat("2000")["ts"], 1000);
    assert_eq!(beat("2501")["refreshed"], true);
}

#[test]
fn release_deletes_only_the_callers_own_claim() {
    let world = World::new();
    claim(&world, "7", "alpha", "1000");
    let error = world.afk(&["release", "--issue", "7", "--instance", "beta"]).error();
    assert!(error.contains("not this fleet's claim") && error.contains("alpha"), "{error}");
    assert!(!world.remote(&["rev-parse", CLAIM_7]).is_empty(), "left alone");

    let released = world.afk(&["release", "--issue", "7", "--instance", "alpha"]).ok();
    assert_eq!(released["released"], true);
    assert_eq!(world.remote(&["for-each-ref", "refs/convoy/claim"]), "");
    // already gone counts as released
    assert_eq!(
        world.afk(&["release", "--issue", "7", "--instance", "alpha"]).ok()["released"],
        true
    );
}

#[test]
fn a_claim_a_peer_took_since_it_was_read_is_left_alone() {
    use afkconvoy::claims::{Scan, delete};
    use afkconvoy::git::Repo;

    let world = World::new();
    let first = claim(&world, "7", "alpha", "1000");
    let repo = Repo::at(&world.clone).unwrap();
    let mut scan = Scan::read(&repo).unwrap();
    // a peer re-stamps the claim after the scan read it
    let theirs = repo.record_commit("convoy:claim instance=beta host=h2 ts=1001").unwrap();
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{theirs}:{CLAIM_7}")]);

    let error = delete(&repo, &mut scan, 7, first["sha"].as_str().unwrap()).unwrap_err();
    assert!(error.to_string().contains("moved since it was read"), "{error}");
    assert_eq!(world.remote(&["rev-parse", CLAIM_7]), theirs);
}

#[test]
fn a_release_that_left_the_ref_on_the_remote_is_an_error() {
    let world = World::new();
    claim(&world, "7", "alpha", "1000");
    world.watch_pushes(Some("convoy/claim"));
    let error = world.afk(&["release", "--issue", "7", "--instance", "alpha"]).error();
    assert!(error.contains("still on the remote"), "{error}");
}

#[test]
fn scan_lists_every_claim_and_heartbeat() {
    let world = World::new();
    claim(&world, "9", "alpha", "1000");
    claim(&world, "7", "beta", "1100");
    let scan = world.afk(&["scan"]).ok();
    let numbers: Vec<_> = scan["claims"].as_array().unwrap().iter().map(|c| &c["number"]).collect();
    assert_eq!(numbers, [7, 9]);
    assert_eq!(scan["claims"][0]["instance"], "beta");
    assert_eq!(scan["claims"][0]["ts"], 1100);
    assert_eq!(scan["heartbeats"], json!({"alpha": 1000, "beta": 1100}));
}

#[test]
fn a_released_claim_is_gone_from_the_next_scan() {
    let world = World::new();
    claim(&world, "7", "alpha", "1000");
    world.afk(&["scan"]).ok();
    world.afk(&["release", "--issue", "7", "--instance", "alpha"]).ok();
    assert_eq!(world.afk(&["scan"]).ok()["claims"], json!([]));
}

#[test]
fn an_unreadable_remote_is_an_error_never_an_empty_list() {
    let world = World::new();
    git(&world.clone, &["remote", "set-url", "origin", "/nowhere/at/all.git"]);
    let error = world.afk(&["scan"]).error();
    assert!(error.contains("git ls-remote"), "{error}");
}

#[test]
fn a_claim_whose_record_cannot_be_read_names_nobody() {
    let world = World::new();
    let head = git(&world.clone, &["rev-parse", "HEAD"]);
    git(&world.clone, &["push", "-q", "origin", &format!("{head}:{CLAIM_7}")]);
    git(&world.clone, &["push", "-q", "origin", &format!("{head}:refs/convoy/claim/not-a-number")]);
    let scan = world.afk(&["scan"]).ok();
    assert_eq!(scan["claims"].as_array().unwrap().len(), 1);
    assert_eq!(scan["claims"][0]["number"], 7);
    assert_eq!(scan["claims"][0]["instance"], json!(null));
    // …which is never mine
    let error = world.afk(&["release", "--issue", "7", "--instance", "alpha"]).error();
    assert!(error.contains("not this fleet's claim"), "{error}");
    let lost = claim(&world, "7", "alpha", "1000");
    assert_eq!(lost["won"], false);
}

#[test]
fn an_instance_id_outside_its_grammar_is_refused() {
    let world = World::new();
    for bad in ["Alpha", "a/b", "a_b", "a..b"] {
        let error = world.afk(&["claim", "--issue", "7", "--instance", bad]).error();
        assert!(error.contains("instance id"), "{error}");
    }
    assert_eq!(world.remote(&["for-each-ref", "refs/convoy"]), "");
}

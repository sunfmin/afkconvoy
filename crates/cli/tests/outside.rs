//! A change made outside this host — on GitHub, or by another host's fleet —
//! as a `run` shows it: at its next tick, named on the line of what that tick
//! did about it.

use crate::common;

use std::time::Duration;

use common::{Running, World};
use serde_json::json;

/// How long a line a wake brings about is waited for. The sleep it ends is
/// 90 seconds at the least, so "within this" means "woken".
const SOON: Duration = Duration::from_secs(20);

/// The lease, and a moment past it for a heartbeat stamped at 1000.
const LATER: &str = "2860"; // 31 minutes after 1000

/// A `run` of two ticks as `alpha` on h1, its clock stopped at 1000.
fn run(world: &World) -> Running {
    world.running(&["--instance", "alpha", "--host", "h1", "--ticks", "2", "--now", "1000"])
}

/// The first tick's line, then — once `between` changed the world — the
/// second's, which a wake starts now.
fn two_ticks(world: &World, between: impl FnOnce()) -> (String, String) {
    by_command(world, || {
        between();
        assert_eq!(world.afk(&["wake"]).ok()["woke"], true);
    })
}

/// The first tick's line, then the second's, which `command` — one that
/// changes fleet state, and so wakes the `run` itself — starts now.
fn by_command(world: &World, command: impl FnOnce()) -> (String, String) {
    let running = run(world);
    let first = running.line(SOON).expect("the first tick");
    command();
    let second = running.line(SOON).expect("the second tick");
    (first, second)
}

#[test]
fn a_claim_another_host_took_over_is_said_lost_at_the_next_tick() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    let (first, second) = by_command(&world, || {
        let from = ["takeover", "--from", "alpha", "--instance", "beta", "--host", "h2"];
        let taken = world.afk(&[&from[..], &["--now", "1060", "--yes"]].concat()).ok();
        assert_eq!(taken["taken"], json!([3]));
    });
    assert!(first.starts_with("tick 1: dispatched #3 · "), "{first}");
    assert_eq!(
        second,
        "tick 2: lost #3 to instance beta on host h2 (takeover); stopped its worker · holding 0 · frontier 0 · \
         next in 90s"
    );
}

#[test]
fn a_claim_taken_as_stale_is_said_lost_past_the_lease() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let (_, second) = by_command(&world, || {
        let from = ["takeover", "--from", "alpha", "--instance", "beta", "--host", "h2"];
        let taken = world.afk(&[&from[..], &["--now", LATER]].concat()).ok();
        assert_eq!(taken["taken"], json!([3]));
    });
    assert!(
        second
            .starts_with("tick 2: lost #3 to instance beta on host h2 (stale: past the lease); stopped its worker · "),
        "{second}"
    );
    assert_eq!(world.orca()["terminals"][0]["open"], false);
}

#[test]
fn an_issue_labelled_ready_on_github_is_dispatched_saying_since_when() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(5, &[]);
    world.issue(7, &[]);
    let (first, second) = two_ticks(&world, || {
        world.with_gh(|gh| {
            gh["issues"][0]["labels"] = json!(["ready-for-agent"]);
            // a change the tick does nothing about
            gh["issues"][1]["labels"] = json!(["bug"]);
        });
    });
    assert!(first.starts_with("tick 1: nothing to do · "), "{first}");
    assert_eq!(
        second,
        "tick 2: dispatched #5 (labelled ready-for-agent since tick 1) · holding 1 · frontier \
         0 · next in 90s"
    );
}

#[test]
fn a_claimed_issue_closed_on_github_is_left_saying_who_closed_it() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    let (_, second) = two_ticks(&world, || {
        world.with_gh(|gh| {
            gh["issues"][0]["state"] = json!("closed");
            gh["issues"][0]["closed_by"] = json!("alice");
        });
    });
    assert_eq!(second, "tick 2: left #3 (closed by @alice) · holding 0 · frontier 0 · next now");
}

#[test]
fn a_blocker_added_on_github_is_named_on_the_skip_with_since_when() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &[]);
    world.issue(5, &[]);
    let (_, second) = two_ticks(&world, || {
        world.with_gh(|gh| {
            gh["issues"][0]["labels"] = json!(["ready-for-agent"]);
            // the rebuild reads no blocker of #3: the dispatch reads it
            gh["edges"]["3"] = json!([5]);
            gh["issues"][0]["blockers_at_rebuild"] = json!([]);
        });
    });
    assert_eq!(
        second,
        "tick 2: skipped #3 (blocked by #5, added since tick 1) · holding 0 · frontier 0 · \
         next in 90s"
    );
}

#[test]
fn a_change_the_tick_does_nothing_about_is_in_no_line() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    world.issue(9, &[]);
    let (_, second) = two_ticks(&world, || {
        world.with_gh(|gh| {
            gh["issues"][1]["labels"] = json!(["ready-for-agent", "epic"]);
            gh["issues"][0]["title"] = json!("renamed");
        });
    });
    assert_eq!(second, "tick 2: nothing to do · holding 1 · frontier 0 · next in 90s");
}

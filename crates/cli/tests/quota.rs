use crate::common::World;
use serde_json::json;

fn primary(world: &World, api: &str) {
    world.with_gh(|gh| {
        gh["quota_fail"] = json!({api: "gh: API rate limit already exceeded"});
    });
}

#[test]
fn graphql_primary_exhaustion_still_dispatches_with_authoritatively_no_open_prs() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    primary(&world, "graphql");
    let started = world
        .afk(&[
            "dispatch",
            "--issue",
            "146",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1000",
        ])
        .ok();
    assert_eq!(started["started"], true, "{started}");
}

#[test]
fn rest_primary_exhaustion_still_reads_the_frontier() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    primary(&world, "core");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 146, "{plan}");
}

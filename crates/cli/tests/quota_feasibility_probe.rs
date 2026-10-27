use crate::common::World;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn exhaust(world: &World, command: &str) {
    let gh = world.root.join("bin/gh");
    let fake = std::fs::read_link(&gh).unwrap();
    std::fs::remove_file(&gh).unwrap();
    let fake = fake.display().to_string().replace('\'', "'\\''");
    std::fs::write(&gh, format!(
        "#!/bin/sh\nif [ \"$1 $2\" = '{command}' ]; then\n  echo 'GraphQL: API rate limit already exceeded' >&2\n  exit 1\nfi\nexec '{fake}' \"$@\"\n"
    )).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn graphql_exhaustion_should_still_allow_a_rebuild() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    exhaust(&world, "api graphql");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 146);
}

#[test]
fn pr_list_exhaustion_should_still_allow_dispatch() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    exhaust(&world, "pr list");
    let started = world.afk(&[
        "dispatch", "--issue", "146", "--instance", "alpha", "--host", "h1", "--now", "1000",
    ]).ok();
    assert_eq!(started["started"], true);
}

#[test]
fn a_native_link_without_a_body_reference_still_holds_an_issue_off() {
    let world = World::new();
    world.issue(146, &["ready-for-agent"]);
    world.pr(70, 146, "work");
    world.with_gh(|gh| {
        gh["prs"][0]["body"] = json!("No closing keyword here.");
        gh["prs"][0]["linked"] = json!([146]);
    });
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"], json!([]));
    assert!(plan["excluded"].as_array().unwrap().iter().any(|row|
        row["number"] == 146 && row["reason"] == "has an open linked PR"
    ), "{plan}");
}

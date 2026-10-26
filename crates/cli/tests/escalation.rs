//! Default escalations wait for the host's watch, with no consult worker.

use crate::common::World;
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now.to_string()]).ok()
}

fn escalated(yaml: &str) -> World {
    let world = World::new();
    world.set_gate_config(&format!("gate_command: ./gate.sh\ngate_seconds: 600\n{yaml}"));
    world.issue(7, &["ready-for-agent", "bug"]);
    tick(&world, 1000);
    world.comment(7, "<!--convoy:verdict phase=escalate-->\nThe schema is ambiguous.");
    tick(&world, 1090);
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    assert!(world.claims().is_empty());
    world
}

#[test]
fn an_unbound_escalation_never_starts_a_consult_and_stays_visible_to_the_watch() {
    for config in ["", "escalate_to: human", "escalate_to: consult"] {
        let world = escalated(config);
        let before = world.orca()["terminals"].as_array().unwrap().len();
        let plan = world.afk(&["plan", "--now", "1180"]).ok();
        assert_eq!(plan["frontier"], json!([]));
        assert_eq!(plan["waiting_on_human"][0]["issue"], 7);
        assert!(
            plan["waiting_on_human"][0]["answer"]
                .as_str()
                .unwrap()
                .contains("--add-label ready-for-agent --remove-label ready-for-human")
        );
        tick(&world, 1180);
        tick(&world, 1270);
        assert!(world.claims().is_empty());
        assert_eq!(world.orca()["terminals"].as_array().unwrap().len(), before);
        assert!(!world.comments(7).join("\n").contains("<!--convoy:consult"));
        let refused = world
            .afk(&[
                "dispatch",
                "--issue",
                "7",
                "--instance",
                "alpha",
                "--host",
                "h1",
                "--now",
                "1300",
            ])
            .error();
        assert!(refused.contains("is escalated"), "{refused}");
        assert!(world.claims().is_empty());
    }
}

#[test]
fn a_watch_handing_an_unbound_issue_back_moves_it_as_a_humans_answer_would() {
    let world = escalated("");
    let waiting = world.afk(&["plan", "--now", "1180"]).ok();
    let answer = waiting["waiting_on_human"][0]["answer"].as_str().unwrap();
    world.comment(7, "The watch checked the repo: use the v2 schema.");
    let result = world.command("sh", &world.clone).args(["-c", answer]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    tick(&world, 1270);
    assert_eq!(world.claims().len(), 1);
    assert_eq!(world.labels(7), ["bug", "ready-for-agent"]);
    // The next ordinary worker's escalation is visible to the watch again.
    world.comment(7, "<!--convoy:verdict phase=escalate-->\nAnother question.");
    tick(&world, 1360);
    let plan = world.afk(&["plan", "--now", "1450"]).ok();
    assert_eq!(plan["waiting_on_human"][0]["issue"], 7);
    assert_eq!(plan["frontier"], json!([]));
}

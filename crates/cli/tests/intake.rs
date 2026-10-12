//! Intake by a worker (docs/adr/0011): `intake` puts a flow label on an issue
//! and leaves a record of who did; the open issues taken in that way are held
//! under the repo's `intake_limit`, counted from those records; and an issue
//! a human labelled is outside all of it.

mod common;

use common::{Answer, World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: done }
```
";

/// A world whose repo is bound to a team with the workflows `feature` and
/// `fix`, and may hold `limit` open issues taken in by workers.
fn bound(limit: u64) -> World {
    let world = World::new();
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", "build\n")]);
    let workflows = [("workflows/feature.md", FEATURE), ("workflows/fix.md", FEATURE)];
    world.repo("acme/team", &[&[("team.md", TEAM)][..], &workflows].concat());
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\nintake_limit: \
         {limit}\n```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// A worker in the role `dev`, on issue 7, takes `issue` in to `workflow`.
fn intake(world: &World, issue: u64, workflow: &str) -> Answer {
    let issue = issue.to_string();
    world.afk(&[
        "intake",
        "--issue",
        &issue,
        "--workflow",
        workflow,
        "--role",
        "dev",
        "--from",
        "7",
    ])
}

fn record(workflow: &str) -> String {
    format!(
        "<!--convoy:intake workflow={workflow} role=dev from=7-->\n**afkconvoy** — a worker in \
         the role `dev`, on #7, took this issue in to the workflow `{workflow}`."
    )
}

/// The count `plan` reports against the limit.
fn planned(world: &World) -> Value {
    world.afk(&["plan"]).ok()["intake"].clone()
}

#[test]
fn an_intake_puts_the_flow_label_on_and_leaves_a_record_of_the_role_and_the_issue() {
    let world = bound(5);
    world.issue(12, &["bug"]);

    let taken = intake(&world, 12, "feature").ok();
    assert_eq!(
        taken,
        json!({"issue": 12, "workflow": "feature", "taken": true, "role": "dev", "from": 7,
            "open": 1, "limit": 5, "woke": false})
    );
    assert_eq!(world.labels(12), ["bug", "flow:feature"]);
    assert_eq!(world.comments(12), [record("feature")]);

    // `show --issue` says how it came in, and the fleet works it like any other.
    let shown = world.afk(&["show", "--issue", "12"]).ok();
    assert_eq!(
        shown["intake"],
        json!({"by": "worker", "role": "dev", "from": 7, "url": "https://gh/c/1001"})
    );
    assert_eq!(shown["position"]["stage"], "build");
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 12, "{plan}");
}

#[test]
fn the_role_and_the_issue_are_read_from_the_worker_s_environment() {
    let world = bound(5);
    world.issue(12, &[]);
    let ran = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["intake", "--issue", "12", "--workflow", "fix"])
        .env("AFKCONVOY_ROLE", "dev")
        .env("AFKCONVOY_ISSUE", "7")
        .output()
        .unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
    assert_eq!(world.comments(12), [record("fix")]);

    // With neither, it is not known who takes the issue in: nothing is written.
    world.issue(13, &[]);
    let nobody = world.afk(&["intake", "--issue", "13", "--workflow", "fix"]);
    assert!(nobody.error().contains("--role"));
    assert_eq!((world.labels(13), world.comments(13)), (vec![], vec![]));
}

#[test]
fn a_workflow_the_team_does_not_have_is_refused() {
    let world = bound(5);
    world.issue(12, &[]);
    world.gh_calls();

    let error = intake(&world, 12, "research").error();
    assert_eq!(error, "the team acme/team has no workflow \"research\": it has feature, fix");
    let wrote = world.gh_calls().into_iter().any(|call| call.contains(&"--method".to_string()));
    assert!(!wrote, "nothing was written");
    assert_eq!((world.labels(12), world.comments(12)), (vec![], vec![]));

    // Nor is a role the team does not have a record of who took it in.
    let args = ["intake", "--issue", "12", "--workflow", "fix", "--role", "ghost", "--from", "7"];
    assert!(world.afk(&args).error().contains("has no role \"ghost\""));
    assert_eq!(world.comments(12), Vec::<String>::new());
}

#[test]
fn an_intake_is_refused_once_the_open_issues_taken_in_by_workers_reach_the_limit() {
    let world = bound(2);
    for issue in [11, 12, 13] {
        world.issue(issue, &[]);
    }
    assert_eq!(intake(&world, 11, "feature").ok()["open"], 1);
    assert_eq!(intake(&world, 12, "fix").ok()["open"], 2);
    assert_eq!(planned(&world), json!({"open": 2, "limit": 2}));

    let error = intake(&world, 13, "feature").error();
    assert!(error.starts_with("the intake limit is reached: 2 open issues"), "{error}");
    assert!(error.contains("`intake_limit` is 2"), "{error}");
    assert_eq!((world.labels(13), world.comments(13)), (vec![], vec![]));

    // The count is of the records on open issues, and kept nowhere: one closes, and
    // there is room again.
    world.close_issue(11);
    assert_eq!(planned(&world), json!({"open": 1, "limit": 2}));
    assert_eq!(intake(&world, 13, "feature").ok()["open"], 2);
    assert_eq!(world.labels(13), ["flow:feature"]);
}

#[test]
fn an_issue_a_human_labelled_is_not_counted_and_is_never_refused() {
    let world = bound(1);
    for issue in [20, 21, 22] {
        world.issue(issue, &["flow:feature"]);
    }
    world.issue(12, &[]);
    assert_eq!(planned(&world), json!({"open": 0, "limit": 1}));

    // Three of a human's stand, and a worker's first still goes in.
    assert_eq!(intake(&world, 12, "feature").ok()["open"], 1);
    assert_eq!(planned(&world), json!({"open": 1, "limit": 1}));

    // At the limit a human labels one more: it is on the frontier like the rest.
    world.issue(23, &["flow:fix"]);
    let shown = world.afk(&["show", "--issue", "23"]).ok();
    assert_eq!(shown["intake"], json!({"by": "human"}));
    let plan = world.afk(&["plan"]).ok();
    let frontier: Vec<u64> = plan["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|at| at["number"].as_u64().unwrap())
        .collect();
    assert_eq!(frontier, [12, 20, 21, 22, 23]);
    assert_eq!(plan["intake"], json!({"open": 1, "limit": 1}));

    // A human who moves a worker's issue to another workflow has taken it in themselves.
    world.with_gh(|gh| {
        for issue in gh["issues"].as_array_mut().unwrap() {
            if issue["number"] == 12 {
                issue["labels"] = json!(["flow:fix"]);
            }
        }
    });
    assert_eq!(world.afk(&["show", "--issue", "12"]).ok()["intake"], json!({"by": "human"}));
    assert_eq!(planned(&world), json!({"open": 0, "limit": 1}));
}

#[test]
fn an_issue_that_follows_the_workflow_already_is_left_as_it_is() {
    let world = bound(1);
    world.issue(12, &[]);
    world.issue(20, &["flow:feature"]);
    intake(&world, 12, "feature").ok();
    world.gh_calls();

    // Its own again, and a human's — at the limit, and neither is refused.
    let again = intake(&world, 12, "feature").ok();
    assert_eq!(
        again,
        json!({"issue": 12, "workflow": "feature", "taken": false,
            "standing": {"by": "worker", "role": "dev", "from": 7, "url": "https://gh/c/1001"}})
    );
    let human = intake(&world, 20, "feature").ok();
    assert_eq!((&human["taken"], &human["standing"]), (&json!(false), &json!({"by": "human"})));
    let wrote = world.gh_calls().into_iter().any(|call| call.contains(&"--method".to_string()));
    assert!(!wrote, "nothing was written");
    assert_eq!(world.comments(12).len(), 1);

    // An issue follows one workflow: it is not taken in to a second.
    let error = intake(&world, 20, "fix").error();
    assert!(error.contains("already follows a workflow (feature)"), "{error}");
    assert_eq!(world.labels(20), ["flow:feature"]);
}

#[test]
fn only_an_open_issue_of_a_bound_repo_is_taken_in() {
    let world = bound(5);
    world.issue(12, &[]);
    world.close_issue(12);
    assert!(intake(&world, 12, "feature").error().contains("issue #12 is closed"));
    assert_eq!(world.comments(12), Vec::<String>::new());

    let unbound = World::new();
    unbound.issue(12, &[]);
    assert!(intake(&unbound, 12, "feature").error().starts_with("this repo is unbound"));
    // …and an unbound repo's plan has no count to report.
    assert_eq!(unbound.afk(&["plan"]).ok().get("intake"), None);
}

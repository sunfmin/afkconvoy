//! `flows`: whoever hands the fleet work asks it how to open an issue the
//! fleet will work — what each workflow is for, the line that opens an issue
//! into it, the labels to keep off — and it creates the labels that line
//! names, so the line works on a repo nobody labelled yet.

use crate::common;

use common::{World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nescalate_label: needs-a-human\nroles:\n  dev: acme/dev\n```\n";
const FIX: &str = "\
# Fix

For a defect: the issue holds a reproduction.

```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: land }
land: { by: dev, do: implement }
```

Not what it is for: this is below the yaml.
";
const NOTE: &str = "\
```yaml
stages:
  - { name: draft, by: human, on: { pass: write } }
  - { name: scrap, by: dev, do: implement, on: { pass: done } }
  - { name: write, by: dev, do: implement, on: { pass: land, reject: scrap } }
land: { by: dev, do: implement }
```
";

/// A world whose repo is bound to a team with the workflows `fix` and `note`.
fn bound() -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    let files = [("team.md", TEAM), ("workflows/fix.md", FIX), ("workflows/note.md", NOTE)];
    world.repo("acme/team", &files);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
                  epic_labels: [epic]\nlock_label: stop\nintake_limit: 4\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

#[test]
fn delivery_creation_preserves_separate_tracker_repo_label_and_selected_workflow() {
    let world = bound();
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, "acme/tracker");
    // Starting a fleet creates its repo label in the tracker.
    world.with_tracker(|gh| gh["labels"].as_array_mut().unwrap().push(json!("repo:acme/dev")));
    let delivery = world
        .afk(&[
            "delivery",
            "--jira",
            "https://example.atlassian.net/browse/ABC-123",
            "--final-target",
            "main",
        ])
        .ok();
    let id = delivery["delivery"].as_str().unwrap();
    let answer = world.afk(&["flows", "--delivery", id]).ok();
    let selected =
        answer["workflows"].as_array().unwrap().iter().find(|w| w["name"] == "note").unwrap();
    let line = selected["line"].as_str().unwrap();
    assert!(line.contains("--repo acme/tracker"), "{line}");
    assert!(line.contains("--label 'repo:acme/dev'"), "{line}");
    assert!(!line.contains("flow:"), "{line}");
    run(&world, line).unwrap();
    let mut number = 0;
    world.with_tracker(|gh| {
        let issue = gh["issues"].as_array().unwrap().last().unwrap();
        assert_eq!(issue["labels"], json!(["repo:acme/dev"]));
        number = issue["number"].as_u64().unwrap();
    });
    let script = selected["intake_line"]
        .as_str()
        .unwrap()
        .replace("<n>", &number.to_string())
        .replacen("afkconvoy", "\"$1\"", 1);
    let output = world
        .command("sh", &world.clone)
        .args(["-c", &script, "sh", env!("CARGO_BIN_EXE_afkconvoy")])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    world.with_tracker(|gh| {
        let issue = gh["issues"].as_array().unwrap().last().unwrap();
        assert_eq!(issue["labels"], json!(["repo:acme/dev", "flow:note"]));
    });
    let shown = world.afk(&["show", "--issue", &number.to_string()]).ok();
    assert_eq!(shown["workflow"], "note");
    assert_eq!(shown["delivery"], id);
    assert_eq!(shown["target_branch"], delivery["branch"]);
    assert_eq!(shown["intake"]["by"], "human");
    assert_eq!(
        world.afk(&["delivery", "--jira", "https://example.atlassian.net/browse/ABC-123"]).ok()["tasks"],
        json!([format!("acme/tracker#{number}")])
    );
}

/// The labels the repo has, on the stand-in GitHub.
fn repo_labels(world: &World) -> Value {
    world.gh()["labels"].clone()
}

/// Runs `line` — one `flows` answered — as a shell does, with a title and a
/// body put in → whether it opened an issue, and what it said if not.
fn run(world: &World, line: &str) -> Result<(), String> {
    write(&world.clone, "body.md", "the body\n");
    let line = line.replace("<title>", "a title").replace("<file>", "body.md");
    let ran = world.command("sh", &world.clone).args(["-c", &line]).output().unwrap();
    if ran.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&ran.stderr).into()) }
}

#[test]
fn a_bound_repo_answers_every_workflow_and_what_it_is_for() {
    let world = bound();
    let answer = world.afk(&["flows"]).ok();
    assert_eq!(answer["team"], "acme/team");
    assert_eq!(
        answer["workflows"],
        json!([
            {"name": "fix", "flow_label": "flow:fix",
             "description": "# Fix\n\nFor a defect: the issue holds a reproduction.",
             "first": {"stage": "build", "by": "dev", "do": "implement"},
             "ends": ["land"],
             "line": "gh issue create --label 'flow:fix' --title '<title>' --body-file <file>"},
            // A file with no prose says nothing of what its workflow is for;
            // this one waits for a human first, and ends two ways.
            {"name": "note", "flow_label": "flow:note", "description": "",
             "first": {"stage": "draft", "by": "human", "do": null},
             "ends": ["land", "done"],
             "line": "gh issue create --label 'flow:note' --title '<title>' --body-file <file>"},
        ])
    );
    // The escalate label is the team's, the others the config's.
    assert_eq!(
        answer["keep_off"],
        json!({"epic_labels": ["epic"], "escalate_label": "needs-a-human", "lock_label": "stop"})
    );
}

#[test]
fn check_warns_of_a_workflow_that_says_nothing_of_what_it_is_for() {
    let world = bound();
    let checked = world.afk(&["check"]);
    assert_eq!(checked.code, 0, "a warning refuses nothing");
    let warned: Vec<&str> = checked.stderr.lines().collect();
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].starts_with("warning: workflows/note.md says nothing of what"), "{warned:?}");
    assert!(warned[0].contains("`afkconvoy flows`"), "{warned:?}");
}

#[test]
fn every_flow_label_the_repo_lacks_is_created_and_the_line_then_works() {
    let world = bound();
    world.with_gh(|gh| gh["labels"] = json!(["Flow:Note", "bug"]));
    // Before: a human's first intake fails, the label being nowhere.
    let line = "gh issue create --label 'flow:fix' --title '<title>' --body-file <file>";
    assert_eq!(run(&world, line).unwrap_err().trim(), "could not add label: 'flow:fix' not found");

    // GitHub tells labels apart without their case: `Flow:Note` is `flow:note`.
    let first = world.afk(&["flows"]).ok();
    assert_eq!(first["created"], json!(["flow:fix"]));
    assert_eq!(repo_labels(&world), json!(["Flow:Note", "bug", "flow:fix"]));
    assert_eq!(first["workflows"][0]["line"], line);
    run(&world, line).unwrap();
    let opened = world.gh()["issues"][0].clone();
    assert_eq!((&opened["title"], &opened["labels"]), (&json!("a title"), &json!(["flow:fix"])));
    // …and the fleet has it to work, at its workflow's first stage.
    assert_eq!(world.afk(&["plan"]).ok()["frontier"][0]["number"], opened["number"]);

    // A second run creates none, and says so.
    world.gh_calls();
    let second = world.afk(&["flows"]).ok();
    assert_eq!(second["created"], json!([]));
    let wrote = world.gh_calls().into_iter().any(|call| call.contains(&"--method".to_string()));
    assert!(!wrote, "nothing was written");
    assert_eq!(second["workflows"], first["workflows"]);
}

#[test]
fn the_intake_count_is_answered_against_the_limit() {
    let world = bound();
    world.issue(7, &["flow:fix"]);
    world.issue(12, &[]);
    assert_eq!(world.afk(&["flows"]).ok()["intake"], json!({"open": 0, "limit": 4}));
    let taken = ["intake", "--issue", "12", "--workflow", "fix", "--role", "dev", "--from", "7"];
    world.afk(&taken).ok();
    // A worker's is counted; the one a human labelled is outside the limit.
    assert_eq!(world.afk(&["flows"]).ok()["intake"], json!({"open": 1, "limit": 4}));
}

#[test]
fn a_team_with_a_fault_creates_no_label() {
    let world = bound();
    let team = world.root.join("github/acme/team");
    write(&team, "workflows/fix.md", &FIX.replace("pass: land", "pass: nowhere"));
    git(&team, &["commit", "-q", "-am", "a fault"]);
    world.with_gh(|gh| gh["labels"] = json!([]));
    let refused = world.afk(&["flows"]);
    assert_eq!(refused.code, 1);
    assert!(refused.json["faults"].as_array().is_some_and(|faults| !faults.is_empty()));
    assert_eq!(repo_labels(&world), json!([]));
}

#[test]
fn an_unbound_repo_answers_its_ready_label_and_creates_it_when_missing() {
    let world = World::new();
    world.with_gh(|gh| gh["labels"] = json!([]));
    let line = "gh issue create --label 'ready-for-agent' --title '<title>' --body-file <file>";
    assert!(run(&world, line).unwrap_err().contains("'ready-for-agent' not found"));

    let first = world.afk(&["flows"]).ok();
    assert_eq!(
        first,
        json!({"team": null, "ready_label": "ready-for-agent", "line": line,
            "keep_off": {"epic_labels": ["epic", "prd"], "escalate_label": "ready-for-human",
                "lock_label": "fleet-lock"},
            "created": ["ready-for-agent"]})
    );
    run(&world, line).unwrap();
    assert_eq!(world.afk(&["plan"]).ok()["frontier"][0]["title"], "a title");
    assert_eq!(world.afk(&["flows"]).ok()["created"], json!([]));
}

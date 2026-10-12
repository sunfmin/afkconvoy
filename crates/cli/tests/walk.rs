//! The tick walks an issue through the stages of its workflow (docs/adr/0010):
//! a verdict ends its stage's claim, and the next tick starts the stage the
//! records then put the issue at — in the same worktree, under a claim of its
//! own, with what was declared so far as the worker's history.

mod common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: done, reject: build }
```
";

/// A world whose repo is bound to a team with the one workflow `feature`,
/// and runs `concurrency` workers at once.
fn bound(concurrency: u64) -> World {
    let world = World::new();
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", "build\n")]);
    world.repo("acme/reviewer", &[(".claude/skills/review/SKILL.md", "review\n")]);
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
         concurrency: {concurrency}\n```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// One tick at `now`, every worker that has not stopped being at work.
fn tick(world: &World, now: u64) -> Value {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            let at_work = terminal["agent"].is_null() || terminal["agent"]["state"] == "working";
            if terminal["open"] == true && !terminal["command"].is_null() && at_work {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
                terminal["lastOutputAt"] = json!(now * 1000);
            }
        }
    });
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

/// What a tick did about issue 7 and the ones after it that came to
/// anything, but for the boards and the `stage:` labels.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let kept = steps.filter(|step| {
        !["board", "stage_label"].contains(&step["do"].as_str().unwrap())
            && step["issue"].as_u64().is_some_and(|issue| issue >= 7)
            && step["outcome"] != "nothing"
    });
    kept.map(|step| {
        format!("{} {} {}", step["do"].as_str().unwrap(), step["issue"], step["outcome"])
    })
    .map(|line| line.replace('"', ""))
    .collect()
}

/// Declares `verdict` for `stage` of `issue`, as its worker would.
fn declare(world: &World, issue: u64, stage: &str, verdict: &str, more: &[&str]) {
    let issue = issue.to_string();
    let line = ["verdict", verdict, "--issue", &issue, "--stage", stage];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let declared = world.afk(&[&line[..], &commits, more].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The commands of the terminals open in `dir`.
fn workers(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).map(|t| t["command"].as_str().unwrap().to_string()).collect()
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

/// The frontier `plan` reports: each issue with the stage it is at.
fn frontier(world: &World) -> Vec<(u64, String)> {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let rows = plan["frontier"].as_array().unwrap().iter();
    rows.map(|row| (row["number"].as_u64().unwrap(), row["stage"].as_str().unwrap().to_string()))
        .collect()
}

/// Why `plan` leaves `issue` off the frontier.
fn excluded(world: &World, issue: u64) -> String {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let rows = plan["excluded"].as_array().unwrap();
    let row = rows.iter().find(|row| row["number"] == issue).unwrap_or(&Value::Null);
    row["reason"].as_str().unwrap_or_default().to_string()
}

/// The worker of every open terminal reports that it stopped at `since`.
fn stopped(world: &World, since: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": since * 1000});
                terminal["lastOutputAt"] = json!(since * 1000);
                terminal["screen"] = json!(["", "Shall I go on? [y/n]", ""]);
            }
        }
    });
}

#[test]
fn a_pass_starts_the_next_stage_s_worker_in_the_same_worktree_and_done_ends_the_workflow() {
    let world = bound(2);
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["ready-for-agent"]);
    assert_eq!(frontier(&world), [(7, "build".to_string())], "no flow label: never touched");
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let dir = worktree(&world, 7);
    assert!(workers(&world, &dir)[0].contains(" AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' "));
    assert!(brief(&dir).contains("None yet: no stage of this issue has declared a verdict."));

    // Opening a PR declares nothing: the issue is as it was, and so is its worker.
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    assert_eq!(did(&tick(&world, 1090)), [] as [&str; 0]);
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);

    // A `pass` ends the stage's claim, and the issue is on the frontier at the
    // stage that follows, its PR open or not.
    declare(&world, 7, "build", "pass", &["--text", "Built: seven.txt holds it."]);
    assert_eq!(did(&tick(&world, 1180)), ["advance 7 done"]);
    assert_eq!(world.claims(), []);
    assert!(world.boards(7)[0].contains("Its stage declared its verdict"), "{:?}", world.boards(7));
    assert_eq!(frontier(&world), [(7, "review".to_string())]);

    // The next tick starts a worker in that stage's role, where the last one worked.
    assert_eq!(did(&tick(&world, 1270)), ["dispatch 7 done"]);
    assert_eq!(worktree(&world, 7), dir);
    let started = workers(&world, &dir);
    assert_eq!(started.len(), 1, "the worker before it is gone: {started:?}");
    assert!(started[0].contains(" AFKCONVOY_STAGE='review' AFKCONVOY_ROLE='reviewer' "));
    assert!(dir.join("seven.txt").is_file());
    let told = brief(&dir);
    assert!(told.contains("- **Stage:** `review`, of the workflow `feature`\n"), "{told}");
    assert!(told.contains("- `build` declared `pass` — https://gh/c/"), "{told}");
    assert!(!told.contains("## Rework"), "{told}");

    // The tick after a start shows the claim on the status board; one more
    // over an unchanged world does nothing.
    assert_eq!(did(&tick(&world, 1300)), [] as [&str; 0]);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);
    let wrote: Vec<_> = world
        .gh_calls()
        .into_iter()
        .filter(|call| call.contains(&"--method".to_string()))
        .collect();
    assert_eq!(wrote, [] as [Vec<String>; 0], "nothing written to GitHub");

    // A `pass` that leads to `done` ends the workflow: the claim is released,
    // and nobody is started on the issue again.
    declare(&world, 7, "review", "pass", &["--text", "It reads well."]);
    assert_eq!(did(&tick(&world, 1450)), ["advance 7 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1540)), [] as [&str; 0]);
    assert_eq!(excluded(&world, 7), "its workflow has ended");
    assert_eq!(world.state(7), "open");
    assert_eq!(world.labels(8), ["ready-for-agent"]);
    assert_eq!(world.comments(8), Vec::<String>::new());
}

#[test]
fn a_reject_starts_the_stage_the_workflow_names_with_the_reason_in_its_brief() {
    let world = bound(2);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);
    tick(&world, 1180);
    declare(&world, 7, "review", "reject", &["--text", "The empty list is not handled.\nAdd it."]);
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);
    assert_eq!(frontier(&world), [(7, "build".to_string())]);

    assert_eq!(did(&tick(&world, 1360)), ["dispatch 7 done"]);
    let dir = worktree(&world, 7);
    assert!(workers(&world, &dir)[0].contains(" AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' "));
    let told = brief(&dir);
    assert!(told.contains("- `review` declared `reject` — https://gh/c/"), "{told}");
    assert!(told.contains("## Rework\n"), "{told}");
    assert!(told.contains("> The empty list is not handled.\n> Add it."), "{told}");
}

#[test]
fn a_flow_label_that_names_no_workflow_escalates_the_issue_once() {
    let world = bound(2);
    world.issue(9, &["flow:chore", "bug"]);
    world.issue(10, &["flow:feature", "flow:chore"]);
    assert_eq!(frontier(&world), []);
    let ticked = tick(&world, 1000);
    assert_eq!(did(&ticked), ["escalate 9 done", "escalate 10 done"], "{ticked}");
    let said = world.comments(9).join("\n");
    assert!(said.contains("flow:chore names no workflow of the team"), "{said}");
    assert!(world.comments(10).join("\n").contains("2 flow labels (flow:feature, flow:chore)"));
    // It is a human's now: no flow label, the team's escalate label, and no claim.
    assert_eq!(world.labels(9), ["bug", "ready-for-human"]);
    assert_eq!(world.labels(10), ["ready-for-human"]);
    assert_eq!(world.claims(), []);
    assert_eq!(ticked["held"], 0);

    world.gh_calls();
    assert_eq!(did(&tick(&world, 1090)), [] as [&str; 0]);
    assert_eq!(world.comments(9).len(), 1);
}

#[test]
fn blocked_parks_the_issue_and_it_returns_at_the_stage_that_parked_it() {
    let world = bound(2);
    world.issue(3, &["flow:feature"]);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);
    tick(&world, 1180);
    declare(&world, 7, "review", "blocked", &["--text", "Needs #3.", "--blocked-by", "3"]);
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);
    assert_eq!(world.edges(7), [3]);
    assert_eq!(world.claims(), [(3, "alpha".to_string())]);
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);

    world.close(3, "completed");
    assert_eq!(frontier(&world), [(7, "review".to_string())]);
    assert_eq!(did(&tick(&world, 1450)), ["dispatch 7 done"]);
    let started = workers(&world, &worktree(&world, 7));
    assert!(started[0].contains(" AFKCONVOY_STAGE='review' "), "{started:?}");
}

#[test]
fn already_satisfied_closes_the_issue() {
    let world = bound(2);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "already-satisfied", &["--text", "main already does this."]);
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert_eq!(world.orca()["worktrees"], json!([]), "a worktree with no work goes");
}

#[test]
fn a_worker_that_stops_with_no_verdict_is_nudged_and_then_escalated() {
    let world = bound(2);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    let dir = worktree(&world, 7);

    stopped(&world, 1000);
    assert_eq!(did(&tick(&world, 1400)), ["advance 7 nudged"]);
    let orca = world.orca();
    let sent = orca["terminals"].as_array().unwrap().last().unwrap()["sent"].as_array().unwrap();
    let nudge = sent.last().unwrap()["text"].as_str().unwrap();
    assert!(nudge.starts_with("You stopped without declaring a verdict."), "{nudge}");
    assert_eq!(workers(&world, &dir).len(), 1);

    // Silent past the nudge: a human's, with what its screen showed.
    assert_eq!(did(&tick(&world, 1800)), ["advance 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(
        said.contains("The worker of the stage `build` stopped with no valid verdict"),
        "{said}"
    );
    assert!(said.contains("Shall I go on? [y/n]"), "{said}");
    assert_eq!(world.claims(), []);
    // No flow label: the fleet's no longer, and at the stage it stopped at.
    assert_eq!(world.labels(7), ["stage:build", "ready-for-human"]);
    assert_eq!(did(&tick(&world, 1890)), [] as [&str; 0]);
}

#[test]
fn a_record_naming_a_stage_the_workflow_no_longer_has_escalates_the_issue() {
    let world = bound(2);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);

    let team = world.root.join("github/acme/team");
    commit(&team, "workflows/feature.md", &FEATURE.replace("build", "make"));
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 1180)), ["escalate 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(said.contains("The workflow changed under this issue"), "{said}");
    assert!(said.contains("`build`"), "{said}");
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    assert!(!world.labels(7).contains(&"flow:feature".to_string()));
    assert_eq!(world.claims(), []);
}

#[test]
fn every_stage_worker_takes_one_slot_of_concurrency() {
    let world = bound(1);
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    let first = tick(&world, 1000);
    assert_eq!(did(&first), ["dispatch 7 done"]);
    assert_eq!((&first["held"], &first["frontier_remaining"]), (&json!(1), &json!(1)));

    // The slot a verdict frees goes to the frontier as it stood: #8, at its
    // first stage. #7 waits at its next one, like any issue.
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    let second = tick(&world, 1090);
    assert_eq!(did(&second), ["advance 7 done", "dispatch 8 done"]);
    assert_eq!(second["held"], 1);
    let third = tick(&world, 1180);
    assert_eq!(did(&third), [] as [&str; 0]);
    assert_eq!((&third["held"], &third["frontier_remaining"]), (&json!(1), &json!(1)));
    assert_eq!(frontier(&world), [(7, "review".to_string())]);
}

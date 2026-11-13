//! A stage splits its issue (docs/adr/0010): it opens children, takes each in
//! to a workflow of its own, and declares `blocked` on them. Nothing but the
//! park carries it: the parent waits with no claim, the children are worked
//! in the order their blockers close, and when the last has closed the stage
//! that split the parent runs again and judges the whole.

use crate::common;

use std::path::{Path, PathBuf};

use common::{Answer, World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: plan
    by: dev
    do: plan
    on: { pass: build }
  - name: build
    by: dev
    do: implement
    on: { pass: done }
```
";
const FIX: &str = "\
```yaml
stages:
  - name: mend
    by: dev
    do: implement
    on: { pass: done }
```
";

/// A world whose repo is bound to a team with the workflows `feature` and
/// `fix`, and may hold `limit` open
/// issues taken in by workers.
fn bound(limit: u64) -> World {
    let world = World::new();
    let skills = [
        (".claude/skills/plan/SKILL.md", "---\nname: plan\ndescription: plan\n---\n"),
        (".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: do\n---\n"),
    ];
    world.repo("acme/dev", &skills);
    let workflows = [("workflows/feature.md", FEATURE), ("workflows/fix.md", FIX)];
    world.repo("acme/team", &[&[("team.md", TEAM)][..], &workflows].concat());
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
         intake_limit: {limit}\n```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.list_checkout(&world.clone);
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

/// What a tick did that came to anything, but for the boards and the
/// `stage:` labels.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let kept = steps.filter(|step| {
        !["board", "stage_label"].contains(&step["do"].as_str().unwrap())
            && step["issue"].is_u64()
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

/// The worker of the stage issue `from` is at opens issue `child` and takes
/// it in to `workflow`, to wait for `blocked_by`.
fn take_in(world: &World, from: u64, child: u64, workflow: &str, blocked_by: &str) -> Answer {
    if !world.gh()["issues"].as_array().unwrap().iter().any(|issue| issue["number"] == child) {
        world.issue(child, &[]);
    }
    let (from, child) = (from.to_string(), child.to_string());
    let line = ["intake", "--issue", &child, "--workflow", workflow, "--role", "dev", "--from"];
    let waits: &[&str] = if blocked_by.is_empty() { &[] } else { &["--blocked-by", blocked_by] };
    world.afk(&[&line[..], &[&from], waits].concat())
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
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

fn at(issue: u64, stage: &str) -> (u64, String) {
    (issue, stage.to_string())
}

/// Issue 7, at `plan`, is split by its worker into `children`, each taken
/// in to `fix`: the tick after parks it and starts a worker on the first.
fn split(world: &World, children: &[u64]) {
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(world, 1000)), ["dispatch 7 done"]);
    for child in children {
        take_in(world, 7, *child, "fix", "").ok();
    }
    let named: Vec<String> = children.iter().map(u64::to_string).collect();
    let more = ["--text", "Split.", "--blocked-by", &named.join(",")];
    declare(world, 7, "plan", "blocked", &more);
    let first = format!("dispatch {} done", children[0]);
    assert_eq!(did(&tick(world, 1090)), ["advance 7 done".to_string(), first]);
}

#[test]
fn split_inherits_delivery_and_parent_restart_and_harness_switch_retain_it() {
    let world = bound(50);
    world.issue(7, &["flow:feature"]);
    let delivery = world
        .afk(&[
            "delivery",
            "--jira",
            "https://example.atlassian.net/browse/ABC-123",
            "--final-target",
            "main",
            "--issue",
            "7",
        ])
        .ok();
    let other = world
        .afk(&[
            "delivery",
            "--jira",
            "https://example.atlassian.net/browse/ABC-456",
            "--final-target",
            "main",
        ])
        .ok();
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let dir = worktree(&world, 7);
    let told = brief(&dir);
    assert!(told.contains(delivery["delivery"].as_str().unwrap()));
    assert!(told.contains("--from 7"));
    let child = take_in(&world, 7, 11, "fix", "").ok();
    assert_eq!(child["delivery"], delivery["delivery"]);
    assert_eq!(child["target_branch"], delivery["branch"]);
    assert_eq!(world.afk(&["show", "--issue", "11"]).ok()["delivery"], delivery["delivery"]);
    // An explicitly named second delivery overrides the worker source.
    world.issue(12, &[]);
    world
        .afk(&[
            "intake",
            "--issue",
            "12",
            "--workflow",
            "fix",
            "--role",
            "dev",
            "--from",
            "7",
            "--delivery",
            other["delivery"].as_str().unwrap(),
            "--blocked-by",
            "11",
        ])
        .ok();
    assert_eq!(world.afk(&["target", "--issue", "12"]).ok()["delivery"], other["delivery"]);
    // A dependency alone supplies no ownership.
    world.issue(13, &[]);
    take_in(&world, 99, 13, "fix", "11").ok();
    assert_eq!(world.afk(&["target", "--issue", "13"]).ok()["target_branch"], "main");
    assert!(world.afk(&["target", "--issue", "13"]).ok().get("delivery").is_none());
    declare(&world, 7, "plan", "blocked", &["--text", "Split.", "--blocked-by", "11"]);
    tick(&world, 1090);
    assert_eq!(world.edges(7), [11]);
    world.with_gh(|gh| {
        for row in gh["issues"].as_array_mut().unwrap() {
            if [11, 12, 13].contains(&row["number"].as_u64().unwrap()) {
                row["state"] = json!("closed");
            }
        }
    });
    tick(&world, 1180);
    let resumed = worktree(&world, 7);
    assert!(brief(&resumed).contains(delivery["delivery"].as_str().unwrap()));
    let switched = world
        .afk(&[
            "switch",
            "--harness",
            "claude-code",
            "7",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1200",
        ])
        .ok();
    assert_eq!(switched["failed"], json!([]), "{switched}");
    assert!(brief(&resumed).contains(delivery["delivery"].as_str().unwrap()));
    assert_eq!(world.afk(&["target", "--issue", "7"]).ok()["target_branch"], delivery["branch"]);
    assert_eq!(
        world.afk(&["delivery", "--jira", "https://example.atlassian.net/browse/ABC-123"]).ok()["tasks"],
        json!(["#11", "#7"])
    );
}

#[test]
fn a_split_parks_the_parent_and_its_children_are_worked_in_the_order_their_blockers_close() {
    let world = bound(50);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);

    // The brief of the stage says how, with the tool's own commands.
    let dir = worktree(&world, 7);
    let told = brief(&dir);
    assert!(told.contains("## Splitting the issue\n"), "{told}");
    assert!(told.contains("the team has `feature`, `fix` —"), "{told}");
    let intake = format!("   cd '{}' && ", dir.display());
    let line = told
        .lines()
        .find(|line| line.contains(" intake ") && line.contains("[--blocked-by"))
        .unwrap_or_default();
    assert!(line.starts_with(&intake), "{line}");
    assert!(line.ends_with(" intake --issue <n> --workflow <workflow> [--blocked-by <n,…>]"));

    // It opens three children, 12 to wait for 11 and 13 for both, each taken
    // in to a workflow of its own, and declares `blocked` on them.
    let first = take_in(&world, 7, 11, "feature", "").ok();
    assert_eq!((&first["taken"], &first["blocked_by"]), (&json!(true), &json!([])));
    assert_eq!(take_in(&world, 7, 12, "fix", "11").ok()["blocked_by"], json!([11]));
    assert_eq!(take_in(&world, 7, 13, "fix", "11,12").ok()["open"], 3);
    assert_eq!((world.edges(12), world.edges(13)), (vec![11], vec![11, 12]));
    let more = ["--text", "Three pieces: #11, #12, #13.", "--blocked-by", "11,12,13"];
    declare(&world, 7, "plan", "blocked", &more);

    // The parent is parked: an edge to each child, no claim, no slot. The one
    // child nothing blocks is worked.
    let parked = tick(&world, 1090);
    assert_eq!(did(&parked), ["advance 7 done", "dispatch 11 done"]);
    assert_eq!(world.edges(7), [11, 12, 13]);
    assert_eq!(world.claims(), [(11, "alpha".to_string())]);
    assert_eq!(parked["held"], 1);
    assert!(world.boards(7)[0].contains("#11"), "{:?}", world.boards(7));
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 1180)), [] as [&str; 0]);

    // Each child follows the workflow it was taken in to.
    let told = brief(&worktree(&world, 11));
    assert!(told.contains("- **Stage:** `plan`, of the workflow `feature`\n"), "{told}");

    // 11 closes: 12 is next, and 13 still waits for it.
    declare(&world, 11, "plan", "already-satisfied", &["--text", "Nothing to do."]);
    assert_eq!(did(&tick(&world, 1270)), ["advance 11 done"]);
    assert_eq!(world.state(11), "closed");
    assert_eq!(frontier(&world), [at(12, "mend")]);
    assert_eq!(did(&tick(&world, 1360)), ["dispatch 12 done"]);
    let told = brief(&worktree(&world, 12));
    assert!(told.contains("- **Stage:** `mend`, of the workflow `fix`\n"), "{told}");
    assert_eq!(frontier(&world), []);

    declare(&world, 12, "mend", "already-satisfied", &["--text", "Nothing to do."]);
    assert_eq!(did(&tick(&world, 1450)), ["advance 12 done"]);
    assert_eq!(frontier(&world), [at(13, "mend")], "the parent waits for the last child");
    assert_eq!(did(&tick(&world, 1540)), ["dispatch 13 done"]);

    // The last child closes: the parent is on the frontier at the stage that
    // split it, and that stage is told the whole is its to judge.
    declare(&world, 13, "mend", "already-satisfied", &["--text", "Nothing to do."]);
    assert_eq!(did(&tick(&world, 1630)), ["advance 13 done"]);
    assert_eq!(world.claims(), []);
    assert_eq!(frontier(&world), [at(7, "plan")]);
    assert_eq!(did(&tick(&world, 1720)), ["dispatch 7 done"]);
    let told = brief(&worktree(&world, 7));
    assert!(told.contains("- **Stage:** `plan`, of the workflow `feature`\n"), "{told}");
    assert!(told.contains("## After a park\n"), "{told}");
    assert!(told.contains("every issue it waited on has closed (#11, #12, #13)."), "{told}");
    assert!(told.contains("> Three pieces: #11, #12, #13."), "{told}");

    // It goes on: the park was no rework, and `pass` leads where it ever did.
    declare(&world, 7, "plan", "pass", &["--text", "The pieces add up; the rest is to build."]);
    assert_eq!(did(&tick(&world, 1810)), ["advance 7 done", "dispatch 7 done"]);
    assert!(brief(&worktree(&world, 7)).contains("- **Stage:** `build`, of the workflow"));
    let shown = world.afk(&["show", "--issue", "7"]).ok();
    assert_eq!(shown["rework"]["had"], 0);
}

#[test]
fn the_stage_that_split_may_end_the_parent_once_its_children_have_closed() {
    let world = bound(50);
    split(&world, &[11]);
    declare(&world, 11, "mend", "already-satisfied", &["--text", "Nothing to do."]);
    assert_eq!(did(&tick(&world, 1180)), ["advance 11 done"]);
    assert_eq!(frontier(&world), [at(7, "plan")]);
    assert_eq!(did(&tick(&world, 1270)), ["dispatch 7 done"]);

    declare(&world, 7, "plan", "already-satisfied", &["--text", "#11 did all of it."]);
    assert_eq!(did(&tick(&world, 1360)), ["advance 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1450)), [] as [&str; 0]);
}

#[test]
fn a_child_may_be_split_in_turn() {
    let world = bound(50);
    split(&world, &[11]);

    // The child's own stage splits it: the parent waits on, a level up.
    take_in(&world, 11, 21, "fix", "").ok();
    declare(&world, 11, "mend", "blocked", &["--text", "Split.", "--blocked-by", "21"]);
    assert_eq!(did(&tick(&world, 1180)), ["advance 11 done", "dispatch 21 done"]);
    assert_eq!((world.edges(7), world.edges(11)), (vec![11], vec![21]));
    assert_eq!(world.claims(), [(21, "alpha".to_string())]);
    let shown = world.afk(&["show", "--issue", "21"]).ok();
    assert_eq!(shown["intake"]["from"], 11);

    // The grandchild closes, and only the child returns; then the parent.
    declare(&world, 21, "mend", "already-satisfied", &["--text", "Nothing to do."]);
    assert_eq!(did(&tick(&world, 1270)), ["advance 21 done"]);
    assert_eq!(frontier(&world), [at(11, "mend")]);
    assert_eq!(did(&tick(&world, 1360)), ["dispatch 11 done"]);
    declare(&world, 11, "mend", "already-satisfied", &["--text", "#21 did all of it."]);
    assert_eq!(did(&tick(&world, 1450)), ["advance 11 done"]);
    assert_eq!(frontier(&world), [at(7, "plan")]);
}

#[test]
fn children_count_against_the_intake_limit() {
    let world = bound(2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    assert_eq!(take_in(&world, 7, 11, "fix", "").ok()["open"], 1);
    assert_eq!(take_in(&world, 7, 12, "fix", "11").ok()["open"], 2);

    // A third is refused, and left as the worker opened it: no edge, no
    // record, no flow label. The parent, a human's, is not in the count.
    let error = take_in(&world, 7, 13, "fix", "12").error();
    assert!(error.starts_with("the intake limit is reached: 2 open issues"), "{error}");
    assert_eq!((world.edges(13), world.labels(13), world.comments(13)), (vec![], vec![], vec![]));
    assert_eq!(world.afk(&["plan"]).ok()["intake"], json!({"open": 2, "limit": 2}));

    // A child that closes makes room for the next.
    world.close(11, "completed");
    assert_eq!(take_in(&world, 7, 13, "fix", "12").ok()["open"], 2);
    assert_eq!(world.edges(13), [12]);
}

#[test]
fn a_child_is_never_on_the_frontier_ahead_of_what_it_waits_for() {
    let world = bound(50);
    world.issue(11, &[]);
    world.issue(12, &[]);
    world.gh_calls();

    // What cannot be waited for is refused before anything is written.
    for (named, why) in [
        ("12", "issue #12 cannot wait for itself"),
        ("99", "--blocked-by names #99, which cannot be read"),
    ] {
        let error = take_in(&world, 7, 12, "fix", named).error();
        assert!(error.starts_with(why), "{error}");
    }
    let wrote = world.gh_calls().into_iter().any(|call| call.contains(&"--method".to_string()));
    assert!(!wrote, "nothing was written");

    // The edge is written before the flow label that puts the issue on the
    // frontier, and one GitHub already records is not written again.
    world.with_gh(|gh| gh["edges"]["12"] = json!([11]));
    world.issue(10, &[]);
    take_in(&world, 7, 12, "fix", "11,10").ok();
    let wrote: Vec<String> = world
        .gh_calls()
        .into_iter()
        .filter(|call| call.contains(&"--method".to_string()))
        .map(|call| call.iter().find(|word| word.contains("/issues/")).cloned().unwrap_or_default())
        .map(|endpoint| endpoint.rsplit('/').next().unwrap_or_default().to_string())
        .collect();
    assert_eq!(wrote, ["blocked_by", "comments", "labels"]);
    assert_eq!(world.edges(12), [11, 10]);
    assert_eq!(world.labels(12), ["flow:fix"]);
}

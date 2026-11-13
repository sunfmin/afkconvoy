//! The fleet lock (docs/adr/0011): an open issue carrying the lock label stops
//! the fleet from working anything else, and closing the last one ends it.

use crate::common;

use std::path::PathBuf;

use common::{World, git, write};
use serde_json::{Value, json};

const AGENT: [&str; 0] = [];

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&[&args[..], &AGENT].concat()).ok()
}

/// What a tick did, but for the status boards it brought up to date.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    steps
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn numbers(rows: &Value) -> Vec<u64> {
    rows.as_array().unwrap().iter().map(|row| row["number"].as_u64().unwrap()).collect()
}

fn reason(plan: &Value, issue: u64) -> String {
    let rows = plan["excluded"].as_array().unwrap();
    let row = rows.iter().find(|row| row["number"] == issue).expect("an excluded issue");
    row["reason"].as_str().unwrap().to_string()
}

fn lock(world: &World, issue: u64, more: &[&str]) -> common::Answer {
    let issue = issue.to_string();
    world.afk(&[&["lock", "--issue", &issue][..], more].concat())
}

fn land(world: &World, dir: &std::path::Path, issue: u64) -> Value {
    world.afk_in(dir, &["land", "--issue", &issue.to_string(), "--now", "7000"]).ok()
}

#[test]
fn with_a_lock_standing_the_frontier_is_the_lock_issues_and_plan_says_which() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    // In an unbound repo a lock issue is worked when it also carries the
    // ready label: this one stands, and nobody is started on it.
    world.issue(8, &["fleet-lock"]);
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["locks"], json!([7, 8]));
    assert_eq!(numbers(&plan["frontier"]), [7]);
    assert_eq!(reason(&plan, 3), "a fleet lock stands (#7, #8)");
    assert_eq!(reason(&plan, 8), "no ready-for-agent label");

    // No lock stands: `plan` says so, and the frontier is what it was.
    world.close_issue(7);
    world.close_issue(8);
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["locks"], json!([]));
    assert_eq!(numbers(&plan["frontier"]), [3]);
}

#[test]
fn running_workers_are_left_alone_and_what_they_declared_is_acted_on_once_the_lock_ends() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    world.issue(5, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent"]);
    let first = tick(&world, 1000);
    assert_eq!(did(&first), ["dispatch 3 done", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(first["locks"], json!([]));

    // The base branch broke. #7 carries the fix, and #3's worker — silent
    // ever since — declared that its issue needs nothing.
    world.issue(7, &["ready-for-agent"]);
    assert_eq!(lock(&world, 7, &[]).ok()["raised"], true);
    world.comment(3, "<!--convoy:verdict phase=already-satisfied-->\nnothing to do");
    world.gh_calls();
    let locked = tick(&world, 1390);
    assert_eq!(
        did(&locked),
        ["dispatch 7 done", "heartbeat done"],
        "the fleet was at its bound: the lock issue does not wait for a slot, and nobody \
         asks after the workers of #3 and #5"
    );
    assert_eq!(locked["locks"], json!([7]));
    assert_eq!((&locked["held"], &locked["frontier_remaining"]), (&json!(3), &json!(0)));
    assert_eq!(world.state(3), "open", "its verdict waits for the lock to end");
    let typed = world.orca_calls().iter().any(|call| common::asked(call) == "terminal send");
    assert!(typed, "the lock issue's worker was handed its brief");

    // Still locked: only the lock issue's worker is asked after (silent a
    // grace period from its start, it is nudged), and #9 stays where it is.
    assert_eq!(did(&tick(&world, 1480)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 1690)), ["judge 7 nothing", "heartbeat done"]);

    // The fix landed: #7 is closed, and the next tick resumes everything.
    world.close_issue(7);
    let resumed = tick(&world, 1780);
    assert_eq!(resumed["locks"], json!([]));
    let did = did(&resumed);
    assert_eq!(did[..2], ["settle 7 done", "judge 3 done"], "{did:?}");
    assert!(did.contains(&"dispatch 9 done".to_string()), "{did:?}");
    assert_eq!(world.state(3), "closed", "what #3's worker declared was acted on");
}

#[test]
fn the_line_of_a_tick_says_that_a_lock_stands_and_which_issue_it_is() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    let paced = ["--ticks", "1", "--second-ms", "0"];
    let (lines, _) = world.afk_lines(&[&args[..], &AGENT, &paced].concat());
    let line = lines.iter().find(|line| line.starts_with("tick 1: ")).expect("a tick's line");
    assert!(
        line.starts_with("tick 1: FLEET LOCK #7 stands: only it is worked · dispatched #7 · "),
        "{line}"
    );
    assert_eq!(world.claims().iter().map(|claim| claim.0).collect::<Vec<_>>(), [7]);
}

#[test]
fn with_a_lock_standing_only_its_own_issue_can_land() {
    let world = World::new();
    world.set_gate("true");
    let seven = world.worker(7, "seven.txt", "7\n");
    let eight = world.worker(8, "eight.txt", "8\n");
    lock(&world, 8, &[]).ok();
    world.watch_pushes(None);

    let refused = land(&world, &seven, 7);
    assert_eq!(refused["outcome"], "locked");
    assert_eq!(refused["locks"], json!([8]));
    assert!(refused["detail"].as_str().unwrap().contains("a fleet lock stands (#8)"));
    assert_eq!(world.pushes(), [] as [String; 0], "nothing joined, and nothing was pushed");

    // The lock issue's own PR lands and closes the issue: it carries the fix.
    assert_eq!(land(&world, &eight, 8)["outcome"], "merged");
    // …and once it has closed, so does every other.
    assert_eq!(world.state(8), "closed");
    assert_eq!(land(&world, &seven, 7)["outcome"], "merged");
}

#[test]
fn lock_raises_a_lock_once_and_leaves_a_record_of_the_role_that_did() {
    let world = World::new();
    world.issue(7, &["bug"]);
    world.issue(9, &["fleet-lock"]);

    let raised = lock(&world, 7, &["--role", "watch"]).ok();
    assert_eq!(
        raised,
        json!({"issue": 7, "label": "fleet-lock", "role": "watch", "raised": true,
            "locks": [7, 9], "woke": false})
    );
    assert_eq!(world.labels(7), ["bug", "fleet-lock"]);
    let comments = world.comments(7);
    assert_eq!(comments.len(), 1);
    assert!(comments[0].starts_with("<!--convoy:lock role=watch-->\n"), "{}", comments[0]);
    assert!(comments[0].contains("a worker in the role `watch` raised a fleet lock"));

    // A lock that stands is not raised again, whoever asks.
    let again = lock(&world, 7, &["--role", "dev"]).ok();
    assert_eq!((&again["raised"], &again["role"]), (&json!(false), &json!("dev")));
    assert_eq!(world.comments(7).len(), 1);
    let human = lock(&world, 9, &[]).ok();
    assert_eq!(human["raised"], false, "a human put the label on it: there is nothing to add");
    assert_eq!(world.comments(9).len(), 0);

    // A worker started in its role is on record without saying so.
    world.issue(11, &[]);
    let args = ["lock", "--issue", "11"];
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args)
        .env("AFKCONVOY_ROLE", "patrolman")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(world.comments(11)[0].starts_with("<!--convoy:lock role=patrolman-->\n"));

    // A closed issue is no lock.
    world.issue(12, &[]);
    world.close_issue(12);
    assert!(lock(&world, 12, &[]).error().contains("issue #12 is closed"));
    assert_eq!(world.labels(12), [] as [String; 0]);
}

#[test]
fn an_unbound_repo_names_its_own_lock_label_and_its_worker_is_on_record_with_no_role() {
    let world = World::new();
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nlock_label: halt\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    world.issue(3, &["ready-for-agent"]);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    let before = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(before["locks"], json!([]), "fleet-lock is not this repo's lock label");

    let raised = lock(&world, 3, &[]).ok();
    assert_eq!((&raised["label"], &raised["role"]), (&json!("halt"), &Value::Null));
    assert!(world.comments(3)[0].starts_with("<!--convoy:lock-->\n"));
    assert!(world.comments(3)[0].contains("a worker in no role raised a fleet lock"));
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["locks"], json!([3]));
    assert_eq!(numbers(&plan["frontier"]), [3]);
}

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
/// `build` names a consult; `review` goes to a human.
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: reviewer, do: rule }
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    escalate: { to: human }
    on: { pass: done, reject: build }
```
";

/// A world whose repo is bound to a team with the one workflow `feature`,
/// and runs three workers at once.
fn bound() -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    let reviewer = [
        (".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n"),
        (".claude/skills/rule/SKILL.md", "---\nname: rule\ndescription: rule\n---\n"),
    ];
    world.repo("acme/reviewer", &reviewer);
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// One tick of a bound repo at `now`, every worker that has not stopped
/// being at work → what it did that came to anything, but for the boards and
/// the `stage:` labels.
fn at_work(world: &World, now: u64) -> Vec<String> {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            let at_work = terminal["agent"].is_null() || terminal["agent"]["state"] == "working";
            if terminal["open"] == true && !terminal["command"].is_null() && at_work {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
                terminal["lastOutputAt"] = json!(now * 1000);
            }
        }
    });
    worked(&tick(world, now))
}

fn worked(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let kept = steps.filter(|step| {
        !["board", "stage_label", "heartbeat"].contains(&step["do"].as_str().unwrap())
            && step["outcome"] != "nothing"
    });
    kept.map(|step| {
        format!("{} {} {}", step["do"].as_str().unwrap(), step["issue"], step["outcome"])
    })
    .map(|line| line.replace('"', ""))
    .collect()
}

/// Declares `verdict` for `stage` of `issue` at `now`, as the stage's worker
/// would.
fn declare(world: &World, now: u64, issue: u64, stage: &str, verdict: &str) {
    world.with_gh(|gh| gh["now"] = json!(now));
    let issue = issue.to_string();
    let line = ["verdict", verdict, "--issue", &issue, "--stage", stage, "--text", "Said."];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let declared = world.afk(&[&line[..], &commits].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
}

/// Closes the lock `number` at `now`.
fn close_lock(world: &World, number: u64, now: u64) {
    world.with_gh(|gh| {
        gh["now"] = json!(now);
        for issue in gh["issues"].as_array_mut().unwrap() {
            if issue["number"] == number {
                issue["state"] = json!("closed");
                issue["closed_at"] = json!(now);
            }
        }
    });
}

fn position(world: &World, issue: u64) -> (String, String, u64) {
    let shown = world.afk(&["show", "--issue", &issue.to_string()]).ok();
    let at = &shown["position"];
    let word = |value: &Value| value.as_str().unwrap().to_string();
    (word(&at["is"]), word(&at["stage"]), shown["rework"]["had"].as_u64().unwrap())
}

/// The brief of the worker last started on `issue`.
fn brief(world: &World, issue: u64) -> String {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    let dir = PathBuf::from(row["path"].as_str().unwrap());
    let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

#[test]
fn once_the_lock_closes_a_stage_that_declared_escalate_runs_again_and_a_pass_stands() {
    let world = bound();
    for issue in [7, 8, 9] {
        world.issue(issue, &["flow:feature"]);
    }
    assert_eq!(at_work(&world, 1000), ["dispatch 7 done", "dispatch 8 done", "dispatch 9 done"]);
    // #8 is at `review`, which goes to a human, before the base branch breaks.
    declare(&world, 1010, 8, "build", "pass");
    assert_eq!(at_work(&world, 1020), ["advance 8 done", "dispatch 8 done"]);

    // The base branch broke, and #20 is the lock. Both stages met the fault
    // and said so; #9 built on, and passed.
    world.issue(20, &["fleet-lock"]);
    declare(&world, 1100, 7, "build", "escalate");
    declare(&world, 1110, 8, "review", "escalate");
    declare(&world, 1120, 9, "build", "pass");
    assert_eq!(at_work(&world, 1200), [] as [&str; 0], "left alone while the lock stands");

    close_lock(&world, 20, 1300);
    let closed = ["advance 7 done", "advance 8 done", "advance 9 done", "dispatch 9 done"];
    assert_eq!(at_work(&world, 1400), closed);
    // Neither is with a consult nor with a human: each is at its stage again,
    // to be performed, and that is no rework.
    assert_eq!(position(&world, 7), ("perform".into(), "build".into(), 0));
    assert_eq!(position(&world, 8), ("perform".into(), "review".into(), 0));
    for issue in [7, 8] {
        let labels = world.labels(issue);
        assert!(labels.contains(&"flow:feature".to_string()), "{labels:?}");
        assert!(!labels.contains(&"ready-for-human".to_string()), "{labels:?}");
        let record = world.comments(issue).pop().unwrap();
        assert!(record.starts_with("<!--convoy:stage stage="), "{record}");
        assert!(record.contains(" verdict=lock-closed by=fleet-->"), "{record}");
        assert!(record.contains("A fleet lock stood (#20) and has closed."), "{record}");
    }
    // A `pass` declared while the lock stood is advanced as ever, and its
    // next stage started with it. A stage that runs again is the next tick's.
    assert_eq!(position(&world, 9), ("perform".into(), "review".into(), 0));
    assert_eq!(world.claims(), [(9, "alpha".to_string())]);

    assert_eq!(at_work(&world, 1410), ["dispatch 7 done", "dispatch 8 done"]);
    for (issue, stage) in [(7, "build"), (8, "review")] {
        let told = brief(&world, issue);
        assert!(told.contains(&format!("- **Stage:** `{stage}`, of the workflow")), "{told}");
        assert!(told.contains("## After a fleet lock\n\nThis stage runs again"), "{told}");
        assert!(told.contains("> A fleet lock stood (#20) and has closed."), "{told}");
    }
    assert!(!brief(&world, 9).contains("## After a fleet lock"));

    // What a stage declares once the lock has closed is carried out as it
    // stands: nothing runs a second time for the same lock.
    declare(&world, 1500, 8, "review", "escalate");
    assert_eq!(at_work(&world, 1510), ["advance 8 done"]);
    assert!(world.labels(8).contains(&"ready-for-human".to_string()));
    assert_eq!(at_work(&world, 1600), [] as [&str; 0]);
}

#[test]
fn a_stage_that_stayed_silent_past_a_nudge_while_a_lock_stood_runs_again() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.with_gh(|gh| gh["now"] = json!(1000));
    assert_eq!(worked(&tick(&world, 1000)), ["dispatch 7 done"]);
    // Its worker stopped with nothing declared, and was told to carry on.
    world.afk(&["nudge", "7", "--instance", "alpha", "--host", "h1", "--now", "1390"]).ok();
    world.issue(20, &["fleet-lock"]);
    assert_eq!(worked(&tick(&world, 1800)), [] as [&str; 0]);

    close_lock(&world, 20, 1900);
    assert_eq!(worked(&tick(&world, 2000)), ["advance 7 done"]);
    assert_eq!(position(&world, 7), ("perform".into(), "build".into(), 0));
    assert!(world.labels(7).contains(&"flow:feature".to_string()));
    assert_eq!(world.claims(), []);
    assert_eq!(worked(&tick(&world, 2010)), ["dispatch 7 done"]);
    assert!(brief(&world, 7).contains("## After a fleet lock"));

    // A later unexplained silence needs diagnosis and cannot consume a breaker.
    tick(&world, 2400);
    tick(&world, 2800);
    assert!(!world.comments(7).iter().any(|c| c.contains("verdict=failed-silent by=fleet")));
    assert_eq!(position(&world, 7).0, "perform");
    let records = world.comments(7).iter().filter(|c| c.contains("verdict=lock-closed")).count();
    assert_eq!(records, 1);
}

#[test]
fn a_red_gate_says_which_lock_stands_and_a_green_one_says_nothing_of_it() {
    let world = World::new();
    let red = world.root.join("red");
    world.set_gate(&format!("test ! -f {}", red.display()));
    world.issue(8, &["fleet-lock"]);
    let green = world.afk(&["gate"]).ok();
    assert_eq!((&green["status"], &green["locks"]), (&json!("green"), &Value::Null));
    assert!(!green["detail"].as_str().unwrap().contains("lock"), "{green}");

    std::fs::write(&red, "").unwrap();
    let answer = world.afk(&["gate"]).ok();
    assert_eq!((&answer["status"], &answer["locks"]), (&json!("red"), &json!([8])));
    let detail = answer["detail"].as_str().unwrap();
    assert!(detail.contains("A fleet lock stands (#8): the base branch may be what is red"));
    assert!(detail.contains("declare nothing of it, stop, and run this again when you are told"));

    // No lock stands: a red gate is the worker's own to fix.
    world.close_issue(8);
    let own = world.afk(&["gate"]).ok();
    assert_eq!((&own["status"], &own["locks"]), (&json!("red"), &Value::Null));
    assert!(own["detail"].as_str().unwrap().contains("fix it, commit, and run this again"));
}

#[test]
fn the_line_of_a_tick_shows_the_claims_a_lock_leaves_alone_apart_from_the_bound() {
    let world = World::new();
    write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\n```\n",
    );
    world.issue(3, &["ready-for-agent"]);
    world.issue(5, &["ready-for-agent"]);
    tick(&world, 1000);
    world.issue(7, &["ready-for-agent", "fleet-lock"]);
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1", "--now", "1010"];
    let paced = ["--ticks", "1", "--second-ms", "0"];
    let (lines, _) = world.afk_lines(&[&args[..], &AGENT, &paced].concat());
    let line = lines.iter().find(|line| line.starts_with("tick 1: ")).expect("a tick's line");
    assert_eq!(
        line,
        "tick 1: FLEET LOCK #7 stands: only it is worked · dispatched #7 · holding 1 · \
         2 left alone · frontier 0 · next in 90s"
    );
}

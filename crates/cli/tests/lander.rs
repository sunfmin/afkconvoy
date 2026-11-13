//! A workflow ends in the landing train (docs/adr/0004, as amended): once the
//! last stage passes to `land`, a worker in the workflow's landing role gates
//! the issue's PR and joins the train — and from there on the train, its
//! worker and a joining worker's silences are what they are in an unbound repo.

use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str = "\
```yaml
roles:
  planner: acme/planner
  dev: acme/dev
  reviewer: acme/reviewer
  lander: acme/lander
```
";
const FEATURE: &str = "\
```yaml
stages:
  - name: plan
    by: planner
    do: plan
    writes: false
    on: { pass: plan-review }
  - name: plan-review
    by: reviewer
    do: review
    writes: false
    on: { pass: build, reject: plan }
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: land, reject: build }
land: { by: lander, do: join }
```
";
const STAGES: [(&str, &str); 4] =
    [("plan", "planner"), ("plan-review", "reviewer"), ("build", "dev"), ("review", "reviewer")];

/// A world whose repo is bound to a team with the one workflow `feature`:
/// four stages, the last of which passes to the landing train.
fn bound() -> World {
    let world = World::new();
    world.repo(
        "acme/planner",
        &[(".claude/skills/plan/SKILL.md", "---\nname: plan\ndescription: plan\n---\n")],
    );
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    world.repo(
        "acme/reviewer",
        &[(".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n")],
    );
    world.repo(
        "acme/lander",
        &[(".claude/skills/join/SKILL.md", "---\nname: join\ndescription: join\n---\n")],
    );
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: \"true\"\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.list_checkout(&world.clone);
    world
}

/// One tick at `now`, every worker that has not stopped being at work.
fn tick(world: &World, now: u64) -> Value {
    tick_as(world, "alpha", now)
}

/// The same, of the fleet instance `instance`.
fn tick_as(world: &World, instance: &str, now: u64) -> Value {
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
    let ticked = world.afk(&["tick", "--instance", instance, "--host", "h1", "--now", &now]).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    ticked
}

/// What a tick did that came to anything, but for the boards, the `stage:`
/// labels and the heartbeat.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let kept = steps.filter(|step| {
        !["board", "stage_label", "heartbeat"].contains(&step["do"].as_str().unwrap())
            && step["outcome"] != "nothing"
    });
    kept.map(|step| {
        let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
        format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
    })
    .collect()
}

/// Declares `pass` for `stage` of issue 7, as its worker would.
fn pass(world: &World, stage: &str) {
    let line = ["verdict", "pass", "--issue", "7", "--stage", stage, "--text", "Done."];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let declared = world.afk(&[&line[..], &commits].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
}

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == 7).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The terminals open in `dir`.
fn terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let rows = orca["terminals"].as_array().unwrap();
    let here = |t: &&Value| t["open"] == true && t["worktreePath"] == dir.to_str().unwrap();
    rows.iter().filter(here).cloned().collect()
}

/// The command of the one worker open in `dir`.
fn worker(world: &World, dir: &Path) -> String {
    let open = terminals(world, dir);
    assert_eq!(open.len(), 1, "one worker at a time: {open:?}");
    open[0]["command"].as_str().unwrap().to_string()
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

fn frontier(world: &World) -> Vec<(u64, String)> {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let rows = plan["frontier"].as_array().unwrap().iter();
    rows.map(|row| (row["number"].as_u64().unwrap(), row["stage"].as_str().unwrap().to_string()))
        .collect()
}

fn status(world: &World) -> String {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let mine = plan["mine"].as_array().unwrap();
    let row = mine.iter().find(|row| row["number"] == 7).expect("a claim of mine");
    row["status"].as_str().unwrap().to_string()
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

/// Walks issue 7 through its four stages, the first started by a tick of its
/// own and each of the others by the tick that ended the one before, the
/// `build` stage opening PR #70 → the time after the last verdict was carried
/// out. Every tick is 90 seconds after the one before.
fn through_the_stages(world: &World) -> u64 {
    world.issue(7, &["flow:feature"]);
    let mut now = 1000;
    let last = STAGES.len() - 1;
    for (nth, (stage, role)) in STAGES.into_iter().enumerate() {
        if nth == 0 {
            assert_eq!(frontier(world), [(7, stage.to_string())]);
            assert_eq!(did(&tick(world, now)), ["dispatch 7 done"], "{stage}");
        }
        let dir = worktree(world);
        let started = worker(world, &dir);
        let facts = format!(" AFKCONVOY_STAGE='{stage}' AFKCONVOY_ROLE='{role}' ");
        assert!(started.contains(&facts), "{started}");
        if stage == "build" {
            // A stage that wrote the PR is told to turn the gate green first.
            let told = brief(&dir);
            assert!(told.contains("turn the gate green before you declare `pass`"), "{told}");
            commit(&dir, "seven.txt", "7\n");
            git(&dir, &["push", "-q", "origin", "HEAD"]);
            world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
            let gate = world.afk_in(&dir, &["gate"]).ok();
            assert_eq!((&gate["status"], &gate["recorded"]), (&json!("green"), &json!(true)));
        } else {
            assert!(!brief(&dir).contains("## The gate"), "{stage} writes nothing");
        }
        pass(world, stage);
        // The join that follows the last stage is no stage's worker: it is
        // the next tick's to start.
        let carried: &[&str] =
            if nth == last { &["advance 7 done"] } else { &["advance 7 done", "dispatch 7 done"] };
        assert_eq!(did(&tick(world, now + 90)), carried, "{stage}");
        assert_eq!(world.claims().len(), carried.len() - 1);
        now += 180;
    }
    now
}

#[test]
fn a_builder_with_an_open_pr_still_needs_its_workflow_and_worker_health() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    pass(&world, "plan");
    tick(&world, 1090);
    pass(&world, "plan-review");
    tick(&world, 1180);
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    stopped(&world, 1180);

    tick(&world, 1480);
    assert_eq!(status(&world), "at_stage");
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);
    assert_eq!(world.claims(), [(7, "alpha".into())]);
    let path = world.cache().join("afkconvoy/worker-health.json");
    let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(book["workers"][0]["path"], json!(dir), "{book}");
    assert_eq!(book["workers"][0]["action"], "observing", "{book}");
}

/// A landing worker's declaration, made while stopped at its prompt.
fn landing_verdict(world: &World, verdict: &str, blocked_by: &[&str]) {
    let line = [
        "verdict",
        verdict,
        "--issue",
        "7",
        "--stage",
        "land",
        "--text",
        "Landing needs an external fix.",
        "--role-commit",
        "r1",
        "--team-commit",
        "t1",
    ];
    assert_eq!(world.afk(&[&line[..], blocked_by].concat()).ok()["declared"], true);
}

fn no_worker_health(world: &World) {
    let file = world.cache().join("afkconvoy/worker-health.json");
    let book: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert!(book["workers"].as_array().unwrap().is_empty(), "{book}");
}

#[test]
fn a_landing_blocked_verdict_parks_and_resumes_landing() {
    for explicit in [false, true] {
        let world = bound();
        let now = through_the_stages(&world);
        tick(&world, now);
        let dir = worktree(&world);
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
        if explicit {
            std::fs::write(dir.join("unfinished.txt"), "landing work").unwrap();
        }
        world.issue(41, &["flow:feature"]);
        // Keep the blocker off this instance's frontier, without changing its
        // eligibility as an issue the fleet can wait for.
        world
            .afk(&["claim", "--issue", "41", "--instance", "beta", "--now", &now.to_string()])
            .ok();
        stopped(&world, now);
        landing_verdict(&world, "blocked", &["--blocked-by", "41"]);
        if explicit {
            let parked = world
                .afk(&[
                    "park",
                    "--issue",
                    "7",
                    "--instance",
                    "alpha",
                    "--now",
                    &(now + 90).to_string(),
                ])
                .ok();
            assert_eq!(parked["action"], "parked", "{parked}");
        } else {
            assert_eq!(did(&tick(&world, now + 90)), ["advance 7 done"]);
        }
        assert_eq!(world.claims(), [(41, "beta".into())]);
        assert_eq!(world.edges(7), [41]);
        assert_eq!(dir.exists(), explicit, "only work held locally keeps the worktree");
        assert!(terminals(&world, &dir).is_empty());
        assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{branch}")]), head);
        assert_eq!(world.gh()["prs"][0]["state"], "open");
        assert!(frontier(&world).is_empty());
        let shown = world.afk(&["show", "--issue", "7"]).ok();
        assert_eq!(shown["position"]["is"], "parked", "{shown}");
        assert_eq!(shown["position"]["stage"], "land");
        assert!(world.boards(7).iter().any(|body| body.contains("**Parked**")));
        tick(&world, now + 180);
        no_worker_health(&world);

        world.close(41, "completed");
        assert_eq!(frontier(&world), [(7, "land".into())]);
        assert_eq!(did(&tick(&world, now + 270)), ["dispatch 7 done"]);
        let dir = worktree(&world);
        assert!(worker(&world, &dir).contains("AFKCONVOY_STAGE='land'"));
        assert_eq!(status(&world), "landing");
    }
}

#[test]
fn a_landing_escalate_verdict_waits_for_a_human_and_a_ruling_resumes_landing() {
    let world = bound();
    let now = through_the_stages(&world);
    tick(&world, now);
    let dir = worktree(&world);
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    stopped(&world, now);
    landing_verdict(&world, "escalate", &[]);
    assert_eq!(did(&tick(&world, now + 90)), ["advance 7 done"]);
    assert!(world.claims().is_empty());
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    assert!(!dir.exists(), "all work is on the remote");
    assert!(terminals(&world, &dir).is_empty());
    assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{branch}")]), head);
    assert_eq!(world.gh()["prs"][0]["state"], "open");
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let waiting = plan["waiting_on_human"].as_array().unwrap();
    assert_eq!(waiting.len(), 1, "{plan}");
    assert_eq!(waiting[0]["issue"], 7);
    let answer = waiting[0]["answer"].as_str().unwrap();
    assert!(answer.contains("afkconvoy rule") && answer.contains("--issue 7"), "{waiting:?}");
    let shown = world.afk(&["show", "--issue", "7"]).ok();
    assert_eq!(shown["position"]["is"], "escalating", "{shown}");
    assert_eq!(shown["position"]["stage"], "land");
    no_worker_health(&world);
    assert!(did(&tick(&world, now + 180)).is_empty());

    let ruled = world.afk(&["rule", "--issue", "7", "--text", "The external fault is fixed."]).ok();
    assert_eq!(ruled["stage"], "land", "{ruled}");
    assert_eq!(frontier(&world), [(7, "land".into())]);
    assert_eq!(did(&tick(&world, now + 270)), ["dispatch 7 done"]);
    let dir = worktree(&world);
    assert!(worker(&world, &dir).contains("AFKCONVOY_STAGE='land'"));
    assert_eq!(world.afk_in(&dir, &["land", "--issue", "7"]).ok()["outcome"], "merged");
}

#[test]
fn an_issue_walks_a_four_stage_workflow_and_its_landing_worker_lands_in_a_turn() {
    let world = bound();
    let now = through_the_stages(&world);
    let dir = worktree(&world);
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    assert_eq!(frontier(&world), [(7, "land".into())]);
    assert_eq!(did(&tick(&world, now)), ["dispatch 7 done"]);
    let started = worker(&world, &dir);
    assert!(started.contains("AFKCONVOY_STAGE='land' AFKCONVOY_ROLE='lander'"), "{started}");
    let brief = brief(&dir);
    assert!(brief.contains("Your ONLY job is to land that PR in a turn."), "{brief}");
    assert!(brief.contains("land --issue 7"));
    assert!(!brief.contains("landing train"));
    assert_eq!(status(&world), "landing");
    let answer = world.afk_in(&dir, &["land", "--issue", "7"]).ok();
    assert_eq!(answer["outcome"], "merged", "{answer}");
    assert_eq!(answer["recorded"], true);
    assert_eq!(answer["merged"], base);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.remote(&["show", "refs/heads/main:seven.txt"]), "7");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/train"]).is_empty());
    // It woke the fleet and stopped at its prompt: settling ends it.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": (now + 60) * 1000});
        }
    });
    assert_eq!(did(&tick(&world, now + 90)), ["settle 7 done"]);
    assert!(world.claims().is_empty());
    assert_eq!(tick(&world, now + 180)["idle"], true);
}

#[test]
fn a_faulted_landing_worker_continues_in_its_role_without_consuming_a_restart() {
    let world = bound();
    let now = through_the_stages(&world);
    tick(&world, now);
    let dir = worktree(&world);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("unfinished.txt"), "landing work").unwrap();
    world.comment(70, "<!--convoy:join restarted=1040-->\nexisting restart evidence");
    world.hold_every_harness();
    let claims = world.claims();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && terminal["worktreePath"] == dir.to_str().unwrap() {
                terminal["agent"] = json!({"state":"idle","error":{"code":"insufficient_quota"}});
            }
        }
    });
    tick(&world, now + 90);
    let started = worker(&world, &dir);
    assert!(started.contains(" AFKCONVOY_STAGE='land' AFKCONVOY_ROLE='lander' "), "{started}");
    assert!(started.contains(" && pi "), "{started}");
    assert_eq!(world.claims(), claims);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(dir.join("unfinished.txt")).unwrap(), "landing work");
    assert_eq!(status(&world), "landing");
    assert!(brief(&dir).contains("**Your role:** `lander`"));
    let record =
        world.comments(70).into_iter().find(|c| c.starts_with("<!--convoy:landing")).unwrap();
    assert!(record.contains("restarted=1040"));
    assert!(record.contains("switched="));
}

#[test]
fn a_pass_to_land_with_no_open_pr_hands_the_issue_to_a_human() {
    let world = bound();
    let now = through_the_stages(&world);
    world.with_gh(|gh| gh["prs"][0]["state"] = json!("closed"));
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, now)), ["escalate 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(said.contains("no open PR closes it: there is nothing to join"), "{said}");
    assert_eq!(world.labels(7), ["stage:land", "ready-for-human"]);
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, now + 90)), [] as [&str; 0]);
}

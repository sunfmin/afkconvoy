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
    world.say_concurrency(&world.clone, 2);
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

/// The last line typed at the worker in `dir`.
fn last_told(world: &World, dir: &Path) -> String {
    let open = terminals(world, dir);
    open[0]["sent"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string()
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

/// The landing worker of issue 7 gates its PR and joins the train, with the
/// commands its brief gives.
fn join(world: &World, now: u64) {
    let dir = worktree(world);
    let gate = world.afk_in(&dir, &["gate", "--train"]).ok();
    assert_eq!((&gate["status"], &gate["recorded"]), (&json!("green"), &json!(true)), "{gate}");
    let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", &now.to_string()]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");
}

fn train_worktree(world: &World) -> PathBuf {
    world.root.join("wt/convoy-train")
}

#[test]
fn a_bound_joined_worker_leaves_health_even_with_an_expired_observation() {
    let world = bound();
    let now = through_the_stages(&world);
    tick(&world, now);
    let dir = worktree(&world);
    stopped(&world, now);
    tick(&world, now + 300);
    let path = world.cache().join("afkconvoy/worker-health.json");
    let mut book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(book["workers"][0]["path"], json!(dir), "{book}");
    book["workers"][0]["attempts"] = json!(3);
    std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
    join(&world, now + 350);

    assert_eq!(did(&tick(&world, now + 4000)), ["tend_train done"]);
    assert_eq!(did(&tick(&world, now + 8000)), [] as [&str; 0]);
    assert_eq!(status(&world), "joined");
    assert_eq!(world.labels(7), ["flow:feature", "stage:land"]);
    assert_eq!(world.claims(), [(7, "alpha".into())]);
    let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(book["workers"].as_array().unwrap().iter().all(|w| w["path"] != json!(dir)), "{book}");
    assert!(!world.root.join("diagnosis-asked").exists());
    let landed = world.afk_in(&train_worktree(&world), &["land", "--train"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    tick(&world, now + 8090);
    assert_eq!(world.state(7), "closed");
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

#[test]
fn a_bound_issues_train_given_up_twice_is_a_fleet_fault_with_no_consult() {
    let world = bound();
    let now = through_the_stages(&world);
    assert_eq!(did(&tick(&world, now)), ["dispatch 7 done"]);
    join(&world, now + 60);
    assert_eq!(did(&tick(&world, now + 90)), ["tend_train done"]);
    for at in [now + 690, now + 1380] {
        let dir = train_worktree(&world);
        // Historical nudge evidence lets an operator abandon the stopped
        // train. New unclassified silence waits for diagnosis instead.
        stopped(&world, at - 590);
        let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
        std::fs::write(notes.join("convoy-nudged"), (at - 300).to_string()).unwrap();
        let abandoned = world
            .afk(&["abandon", "--instance", "alpha", "--host", "h1", "--now", &at.to_string()])
            .ok();
        assert_eq!(abandoned["action"], "abandoned");
        if at == now + 690 {
            join(&world, at + 10);
            assert_eq!(did(&tick(&world, at + 90)), ["tend_train done"]);
        }
    }
    assert_eq!(world.claims(), []);
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    let comments = world.comments(7);
    let escalation = comments.iter().find(|body| body.contains("convoy:escalation")).unwrap();
    assert!(escalation.contains("Fleet fault:") && escalation.contains("the fix is on the host"));
    world.orca_calls();
    assert_eq!(did(&tick(&world, now + 1470)), [] as [&str; 0]);
    assert_eq!(world.claims(), [], "no consult claim is taken");
    assert_eq!(world.comments(7), comments);
    assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"));
    let doctor = world.afk(&["doctor", "--now", &(now + 1500).to_string()]).json;
    let why = doctor["waiting_on_human"][0]["why"].as_str().unwrap();
    assert!(why.contains("Fleet fault:") && why.contains("the fix is on the host"), "{doctor}");
}

#[test]
fn an_issue_walks_a_four_stage_workflow_to_the_base_branch() {
    let world = bound();
    let now = through_the_stages(&world);
    let dir = worktree(&world);
    let base = world.remote(&["rev-parse", "refs/heads/main"]);

    // The last `pass` led to `land`: the issue is on the frontier for its join.
    assert_eq!(world.labels(7), ["flow:feature", "stage:land"]);
    assert_eq!(frontier(&world), [(7, "land".to_string())]);

    // The next tick starts a worker in the workflow's landing role, where the
    // stages worked, with the join-only brief and that role's landing skill.
    let ticked = tick(&world, now);
    assert_eq!(did(&ticked), ["dispatch 7 done"]);
    assert_eq!(ticked["held"], 1, "a landing worker takes a slot");
    assert_eq!(worktree(&world), dir);
    let started = worker(&world, &dir);
    let role_dir = world.cache().join("afkconvoy/copies/acme/lander");
    assert!(started.starts_with(&format!("cd '{}/", role_dir.display())), "{started}");
    assert!(started.contains(" AFKCONVOY_STAGE='land' AFKCONVOY_ROLE='lander' "), "{started}");
    assert!(started.contains(" AFKCONVOY_SKILL='join' AFKCONVOY_WRITES='true' "), "{started}");
    let told = brief(&dir);
    let own = format!("cd '{}' && {}", dir.display(), env!("CARGO_BIN_EXE_afkconvoy"));
    for said in [
        "PR #70 is open for it".to_string(),
        "Your ONLY job is to get that PR on the train.".to_string(),
        "**Your role:** `lander`, which you were started in. Perform its skill `join`".to_string(),
        // It gates and joins with the commands every worker does…
        format!("   {own} gate --train\n"),
        format!("   {own} land --issue 7\n"),
        format!("```bash\n{own} wake\n```"),
        // …and a conflict on joining is its own to resolve.
        "On `\"conflict\"` the train was merged into your branch and left in\n   progress: \
         resolve every file it lists"
            .to_string(),
    ] {
        assert!(told.contains(&said), "{said}\n{told}");
    }
    assert!(!told.contains("Declare a verdict"), "{told}");
    assert_eq!(status(&world), "joining");
    assert_eq!(did(&tick(&world, now + 30)), [] as [&str; 0]);

    // It joins: the train is tended as any train is.
    join(&world, now + 60);
    assert_eq!(status(&world), "joined");
    assert_eq!(did(&tick(&world, now + 90)), ["tend_train done"]);
    let train = worker(&world, &train_worktree(&world));
    assert!(!train.contains("AFKCONVOY_ROLE"), "the train worker has no role: {train}");
    // A defect of the base branch it finds is taken in to the team's workflow: in a
    // bound repo the ready label dispatches nothing.
    let told = brief(&train_worktree(&world));
    let intake = format!("`{} intake --train --issue <n>", env!("CARGO_BIN_EXE_afkconvoy"));
    assert!(told.contains(&intake), "{told}");
    assert!(told.contains("to the team's first workflow,\n   `feature`"), "{told}");
    assert!(!told.contains("--label ready-for-agent"), "{told}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base, "nothing landed yet");

    // The train worker lands it: the issue closes, and the workflow has ended.
    let landed = world.afk_in(&train_worktree(&world), &["land", "--train"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    assert_eq!(world.remote(&["show", "refs/heads/main:seven.txt"]), "7");
    assert_eq!(did(&tick(&world, now + 180)), ["settle 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.gh()["prs"][0]["state"], "merged");
    assert_eq!(world.claims(), []);
    assert!(world.boards(7)[0].contains("#70"), "{:?}", world.boards(7));
    let orca = world.orca();
    let left: Vec<&Value> =
        orca["worktrees"].as_array().unwrap().iter().filter(|w| w["linkedIssue"] == 7).collect();
    assert_eq!(left, [] as [&Value; 0], "its worktree goes with it");

    // Nothing is left to do, and a tick over the unchanged world does nothing.
    let after = tick(&world, now + 270);
    assert_eq!(did(&after), [] as [&str; 0]);
    assert_eq!((&after["held"], &after["frontier_remaining"]), (&json!(0), &json!(0)));
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
    assert_eq!(status(&world), "joining");
    assert!(brief(&dir).contains("**Your role:** `lander`"));
    let record = world.comments(70).into_iter().find(|c| c.starts_with("<!--convoy:join")).unwrap();
    assert!(record.contains("restarted=1040"));
    assert!(record.contains("switched="));
}
#[test]
fn a_pr_taken_off_an_abandoned_train_is_joined_again_by_a_worker_in_the_same_role() {
    let world = bound();
    let now = through_the_stages(&world);
    assert_eq!(did(&tick(&world, now)), ["dispatch 7 done"]);
    let dir = worktree(&world);
    join(&world, now + 50);
    stopped(&world, now + 50);
    assert_eq!(did(&tick(&world, now + 90)), ["tend_train done"]);

    // The train worker stays silent: nudged once, then the train is given up.
    stopped(&world, now + 90);
    assert_eq!(did(&tick(&world, now + 390)), [] as [&str; 0]);
    let train_dir = world.root.join("wt/convoy-train");
    let train_git = PathBuf::from(git(&train_dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::write(train_git.join("convoy-nudged"), (now + 390).to_string()).unwrap();
    std::fs::remove_file(
        PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]))
            .join("convoy-worker-prompt.md"),
    )
    .unwrap();
    let abandoned = world
        .afk(&["abandon", "--instance", "alpha", "--host", "h1", "--now", &(now + 690).to_string()])
        .ok();
    assert_eq!(abandoned["action"], "abandoned");
    assert_eq!(status(&world), "joining");
    assert_eq!(world.gh()["prs"][0]["state"], "open");

    // Its landing worker is told to join the next train, on its role's brief.
    assert!(last_told(&world, &dir).contains("join the next one now"));
    let told = brief(&dir);
    assert!(told.contains("PR #70 is open for it"), "{told}");
    assert!(told.contains("**Your role:** `lander`"), "{told}");

    // With no worker left there to tell, the next tick starts one — in the same role.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == dir.to_str().unwrap() {
                terminal["open"] = json!(false);
            }
        }
    });
    assert_eq!(did(&tick(&world, now + 780)), ["watch 7 continued"]);
    let started = worker(&world, &dir);
    assert!(started.contains(" AFKCONVOY_STAGE='land' AFKCONVOY_ROLE='lander' "), "{started}");
    join(&world, now + 800);
    assert_eq!(status(&world), "joined");
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

#[test]
fn a_join_made_before_its_claim_is_taken_over_is_still_the_claim_s() {
    let world = bound();
    let now = through_the_stages(&world);
    assert_eq!(did(&tick(&world, now)), ["dispatch 7 done"]);
    join(&world, now + 60);
    assert_eq!(did(&tick(&world, now + 90)), ["tend_train done"]);
    let landed = world.afk_in(&train_worktree(&world), &["land", "--train"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");

    // The claim changes hands after the landing, by a clock past the one the
    // landing was made by: it is still the claim the PR was joined under.
    let later = 4_000_000_000u64;
    let from = ["takeover", "--from", "alpha", "--instance", "beta", "--host", "h1", "--now"];
    let taken = world.afk(&[&from[..], &[&later.to_string()]].concat()).ok();
    assert_eq!(taken["taken"], json!([7]), "{taken}");
    assert_eq!(did(&tick_as(&world, "beta", later + 60)), ["settle 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
}

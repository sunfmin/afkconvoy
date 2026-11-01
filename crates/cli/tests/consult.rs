//! A consult rules before a human is asked (docs/adr/0011): the `escalate` of
//! a stage that names one starts a worker in that role, which rules — and the
//! stage that asked runs again under the ruling — or lets the escalation
//! through to a human. It writes no code, and the chain is one role long.

use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

/// A team whose `dev` runs opus and whose `reviewer` runs sonnet, each alone.
const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\nroster:\n  \
                    dev: { run: [{ harness: claude-code, model: opus }] }\n  \
                    reviewer: { run: [{ harness: claude-code, model: sonnet }] }\n```\n";
/// `build` names a consult; `review` names none.
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
    on: { pass: done, reject: build }
```
";

/// A world whose repo is bound to a team with the one workflow `feature`,
/// and runs `concurrency` workers at once.
fn bound(feature: &str, concurrency: u64) -> World {
    let began = std::time::Instant::now();
    let world = World::new();
    world.reuse_repository_templates();
    world.kit_researched_models("claude-code", &["opus", "sonnet"]);
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    let reviewer = [
        (".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n"),
        (".claude/skills/rule/SKILL.md", "---\nname: rule\ndescription: rule\n---\n"),
    ];
    world.repo("acme/reviewer", &reviewer);
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", feature)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.say_concurrency(&world.clone, concurrency);
    common::fixture_timing("consult bound setup", began);
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
    world.afk(&["tick", "--instance", instance, "--host", "h1", "--now", &now]).ok()
}

/// The fleet instance `beta` takes every claim of `alpha` by hand at `now`.
fn taken_over(world: &World, now: u64) {
    let now = now.to_string();
    let from = ["takeover", "--from", "alpha", "--instance", "beta", "--host", "h1", "--yes"];
    let taken = world.afk(&[&from[..], &["--now", &now]].concat()).ok();
    assert_eq!((&taken["taken"], &taken["lost"]), (&json!([7]), &json!([])), "{taken}");
}

/// The worker of the last terminal opened reports that it stopped at `at`.
fn stop(world: &World, at: u64) {
    world.with_orca(|orca| {
        let terminal = orca["terminals"].as_array_mut().unwrap().last_mut().unwrap();
        terminal["agent"] = json!({"state": "idle", "stateStartedAt": at * 1000});
    })
}

/// How each stage record of issue 7 reads.
fn reads(world: &World) -> Vec<String> {
    let read = shown(world);
    let records = read["records"].as_array().unwrap().iter();
    records.map(|record| record["reads"].as_str().unwrap().to_string()).collect()
}

/// Issue 7 of `world`, its `build` dispatched by `alpha`, having declared
/// `escalate` and stopped — and no tick having read that.
fn declared_unread(world: &World) {
    world.issue(7, &["flow:feature"]);
    let first = tick(world, 1000);
    assert_eq!(did(&first), ["dispatch 7 done"], "{first}");
    declare(world, "build", "escalate", "Which schema: v1 or v2?");
    stop(world, 1010);
}

/// What `declared_unread` comes to once the verdict is read: the claim is
/// ended and the issue is with its consult, nothing handed to a human.
fn read_as_declared(world: &World) {
    assert_eq!(world.claims(), []);
    assert_eq!(shown(world)["position"]["is"], "consult");
    assert_eq!(reads(world), ["verdict"]);
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);
    assert_eq!(started(world).len(), 1, "nobody was started in its place");
}

/// What a tick did that came to anything, but for the boards and the
/// `stage:` labels.
fn did(ticked: &Value) -> Vec<String> {
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

const COMMITS: [&str; 4] = ["--role-commit", "r1", "--team-commit", "t1"];

/// Declares `verdict` for `stage` of issue 7, as the stage's worker would.
fn declare(world: &World, stage: &str, verdict: &str, text: &str) {
    let line = ["verdict", verdict, "--issue", "7", "--stage", stage, "--text", text];
    let declared = world.afk(&[&line[..], &COMMITS].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
}

/// Rules on the `escalate` of `build` of issue 7, as its consult would.
fn rule(world: &World, text: &str) -> Value {
    let line = ["rule", "--issue", "7", "--stage", "build", "--text", text];
    world.afk(&[&line[..], &COMMITS].concat()).ok()
}

/// Lets the `escalate` of `build` of issue 7 through, as its consult would:
/// with what the fleet put in its environment.
fn let_through(world: &World, text: &str) -> Value {
    let line = ["verdict", "escalate", "--issue", "7", "--stage", "build", "--text", text];
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(line)
        .env("AFKCONVOY_ROLE_COMMIT", "r1")
        .env("AFKCONVOY_TEAM_COMMIT", "t1")
        .env("AFKCONVOY_CONSULT", "true")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == 7).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The commands of every terminal ever opened.
fn started(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap().iter();
    terminals.filter_map(|t| t["command"].as_str().map(str::to_string)).collect()
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

fn shown(world: &World) -> Value {
    world.afk(&["show", "--issue", "7"]).ok()
}

/// Issue 7 of `world`, its `build` having declared `escalate` and a tick
/// having read that.
fn asked(world: &World) {
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(world, 1000)), ["dispatch 7 done"]);
    declare(world, "build", "escalate", "Which schema: v1 or v2?\nThe issue names neither.");
    assert_eq!(did(&tick(world, 1090)), ["advance 7 done"]);
}

#[test]
fn an_escalate_starts_the_consult_its_stage_names_with_the_history_and_the_reason() {
    let world = bound(FEATURE, 2);
    asked(&world);
    // The issue is still the fleet's: nothing was handed to a human.
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);
    assert_eq!(world.claims(), []);
    assert_eq!(shown(&world)["position"]["is"], "consult");

    let ticked = tick(&world, 1180);
    assert_eq!(did(&ticked), ["dispatch 7 done"]);
    let commands = started(&world);
    let consult = commands.last().unwrap();
    for said in [
        "/copies/acme/reviewer/",
        "AFKCONVOY_STAGE='build'",
        "AFKCONVOY_ROLE='reviewer'",
        "AFKCONVOY_SKILL='rule'",
        "AFKCONVOY_CONSULT='true'",
    ] {
        assert!(consult.contains(said), "{said} in {consult}");
    }
    // A consult runs its own role, not the stage's. Neither command carries a
    // model flag: no model label is followed, and a range that holds one
    // model does not name it.
    assert!(consult.contains("/copies/acme/reviewer/"), "{consult}");
    assert!(!commands[0].contains("--model"), "{}", commands[0]);
    assert!(!consult.contains("--model"), "{consult}");

    let dir = worktree(&world);
    let told = brief(&dir);
    let own = format!("cd '{}' && afkconvoy", dir.display());
    for said in [
        "started in your role as a **consult**",
        "- **Stage that asked:** `build`, of the workflow `feature`\n",
        "- **Skill to perform:** `rule`, of the role you were started in (`reviewer`)\n",
        "- `build` declared `escalate` — https://gh/c/",
        "> Which schema: v1 or v2?\n> The issue names neither.",
        &format!("{own} rule --issue 7 --stage 'build' --text-file <file>"),
        &format!("{own} verdict --issue 7 --stage 'build' escalate --text-file <file>"),
    ] {
        assert!(told.contains(said), "{said}\n{told}");
    }
    // Each command it is given is run in its worktree.
    assert_eq!(common::refusing_a_workers_worktree(&told), [] as [String; 0]);

    // A second tick over an unchanged world does nothing.
    assert_eq!(did(&tick(&world, 1270)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), 2);
}

#[test]
fn an_escalate_with_an_outside_blocker_starts_its_consult_and_names_the_blocker() {
    let world = bound(FEATURE, 2);
    declared_unread(&world);
    world.with_gh(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 931, "state": "open", "repo": "acme/kernel", "labels": []}
        ])
    });
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 7, "{plan}");
    assert_eq!(plan["findings"].as_array().unwrap().len(), 1);
    assert_eq!(did(&tick(&world, 1180)), ["dispatch 7 done"]);
    let told = brief(&worktree(&world));
    assert!(told.contains("acme/kernel#931"), "{told}");
    assert!(told.contains("outside the tracker"), "{told}");
    assert!(told.contains("gh issue transfer 931"), "{told}");
    assert!(told.contains("Which schema: v1 or v2?"), "{told}");
}

#[test]
fn a_worked_blocker_holds_a_consult_even_when_an_outside_blocker_has_the_same_number() {
    let world = bound(FEATURE, 2);
    declared_unread(&world);
    world.with_gh(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 41, "state": "open", "labels": []},
            {"number": 41, "state": "open", "repo": "acme/kernel", "labels": []}
        ])
    });
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["frontier"], json!([]), "{plan}");
    assert_eq!(did(&tick(&world, 1180)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), 1);
}

#[test]
fn a_declared_human_hand_off_with_an_outside_blocker_escalates() {
    let feature =
        FEATURE.replace("escalate: { to: reviewer, do: rule }", "escalate: { to: human }");
    let world = bound(&feature, 2);
    declared_unread(&world);
    world.with_gh(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 931, "state": "open", "repo": "acme/kernel", "labels": []}
        ])
    });
    tick(&world, 1090);
    assert!(with_a_human(&world));
    assert_eq!(world.claims(), []);
}

#[test]
fn an_unlabelled_tracker_blocker_does_not_hold_its_consult() {
    let world = bound(FEATURE, 2);
    world.name_origin(&world.clone, "acme/app");
    world.say_tracker(&world.clone, "acme/tracker");
    world.with_tracker(|gh| {
        gh["issues"].as_array_mut().unwrap().push(json!({
            "number": 7, "title": "needs a capability", "state": "open",
            "labels": ["repo:acme/app", "flow:feature"]
        }))
    });
    let first = tick(&world, 1000);
    assert_eq!(did(&first), ["dispatch 7 done"], "{first}");
    declare(&world, "build", "escalate", "Who will work the kernel blocker?");
    stop(&world, 1010);
    world.with_tracker(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 91, "state": "open", "repo": "acme/tracker", "labels": []}
        ])
    });
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 7, "{plan}");
    assert!(plan["findings"][0]["what"].as_str().unwrap().contains("acme/tracker#91"));
    assert_eq!(did(&tick(&world, 1180)), ["dispatch 7 done"]);
    let told = brief(&worktree(&world));
    assert!(told.contains("acme/tracker#91"), "{told}");
    assert!(told.contains("no single repo: label"), "{told}");
}

#[test]
fn a_consult_s_ruling_is_a_stage_record_and_the_stage_that_asked_runs_again_under_it() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    let ruled = rule(&world, "Keep the v1 schema.");
    assert_eq!((&ruled["declared"], &ruled["verdict"]), (&json!(true), &json!("ruling")));
    let record = world.comments(7).pop().unwrap();
    assert!(record.starts_with("<!--convoy:stage stage=build verdict=ruling by=consult "));
    assert!(
        record.contains("the consult of the stage `build` ruled: the stage runs again"),
        "{record}"
    );

    // The tick that reads the ruling starts the stage that asked again.
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done", "dispatch 7 done"]);
    let read = shown(&world);
    assert_eq!(
        (&read["position"]["is"], &read["position"]["stage"]),
        (&json!("perform"), &json!("build"))
    );
    // Each consult counts toward the issue's rework in total.
    assert_eq!(read["rework"]["had"], 1);
    let last = read["records"].as_array().unwrap().last().unwrap();
    assert_eq!((&last["by"], &last["reads"]), (&json!("consult"), &json!("verdict")));
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);

    let again = started(&world).pop().unwrap();
    assert!(
        again.contains("AFKCONVOY_SKILL='implement'")
            && again.contains("AFKCONVOY_CONSULT='false'"),
        "{again}"
    );
    let told = brief(&worktree(&world));
    assert!(
        told.contains("the consult of the stage `build` ruled: the stage runs again"),
        "{told}"
    );
    assert!(told.contains("## Ruling\n\nThis stage's consult ruled on what it handed on."));
    assert!(told.contains("> Keep the v1 schema."), "{told}");
    assert_eq!(did(&tick(&world, 1450)), [] as [&str; 0]);
}

#[test]
fn a_consult_s_own_escalate_always_goes_to_a_human() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    let through = let_through(&world, "Not mine to rule: the schema is a product decision.");
    assert_eq!(through["declared"], true, "{through}");
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);

    let said = world.comments(7).join("\n");
    assert!(said.contains("the consult of the stage `build` declared `escalate`"), "{said}");
    assert!(
        said.contains(
            "The consult of the stage `build` let it through.\n\nIts stage record: \
             https://gh/c/"
        ),
        "{said}"
    );
    let labels = world.labels(7);
    assert!(labels.contains(&"ready-for-human".to_string()), "{labels:?}");
    assert!(!labels.contains(&"flow:feature".to_string()), "{labels:?}");
    assert_eq!(world.claims(), []);
    // The chain is one role long: nobody else is started on it.
    let before = started(&world).len();
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), before);
}

#[test]
fn a_stage_that_says_to_a_human_escalates_to_a_human_as_before() {
    let human = "    escalate: { to: human }\n";
    let world = bound(&FEATURE.replace("    escalate: { to: reviewer, do: rule }\n", human), 2);
    asked(&world);
    let said = world.comments(7).join("\n");
    assert!(
        said.contains("The stage `build` declared `escalate`, which it hands to a human."),
        "{said}"
    );
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1180)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), 1);
}

#[test]
fn a_stage_that_names_no_consult_escalates_for_the_watch() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, "build", "pass", "Built.");
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done", "dispatch 7 done"]);
    declare(&world, "review", "escalate", "Two readings; which one?");
    assert_eq!(did(&tick(&world, 1180)), ["advance 7 done"]);
    assert!(with_a_human(&world));
    assert!(world.claims().is_empty());
    assert!(did(&tick(&world, 1270)).is_empty());
    assert_eq!(started(&world).len(), 2, "no fallback consult starts");
    let waiting = world.afk(&["plan", "--now", "1270"]).ok();
    assert_eq!(waiting["waiting_on_human"][0]["issue"], 7);
    let answer =
        format!("afkconvoy rule -C '{}' --issue 7 --text-file <file>", world.clone.display());
    assert_eq!(waiting["waiting_on_human"][0]["answer"], answer);
    // A watch answers using the same command as a human, without consult facts.
    world.afk(&["rule", "--issue", "7", "--text", "Use the existing v2 schema."]).ok();
    assert_eq!(did(&tick(&world, 1360)), ["dispatch 7 done"]);
    assert_eq!(shown(&world)["position"]["stage"], "review");
    assert_eq!(shown(&world)["rework"]["had"], 0);
}

#[test]
fn a_consult_s_ruling_past_the_rework_breaker_escalates_with_it_listed() {
    let world = bound(&FEATURE.replacen("```yaml\n", "```yaml\nmax_rework: 0\n", 1), 2);
    asked(&world);
    tick(&world, 1180);
    rule(&world, "Keep the v1 schema.");
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(said.contains("The breaker `max_rework` tripped"), "{said}");
    assert!(said.contains("1. the consult of `build` declared `ruling` — https://gh/c/"), "{said}");
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
}

#[test]
fn a_consult_that_leaves_a_commit_has_failed_and_the_issue_escalates() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    // Past its guard, in a script: the consult fixes what it was asked about.
    let dir = worktree(&world);
    let before = git(&dir, &["rev-parse", "HEAD"]);
    let after = commit(&dir, "schema.txt", "v1\n");
    rule(&world, "Keep the v1 schema; I wrote it.");
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);

    let said = world.comments(7).join("\n");
    let why = format!(
        "A consult writes no code, and the consult of the stage `build` left a commit on the \
         branch: it started at {before} and the branch is now at {after}."
    );
    assert!(said.contains(&why), "{said}");
    let labels = world.labels(7);
    assert!(labels.contains(&"ready-for-human".to_string()), "{labels:?}");
    assert!(!labels.contains(&"flow:feature".to_string()), "{labels:?}");
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);
}

#[test]
fn a_consult_takes_one_slot_of_concurrency() {
    let world = bound(FEATURE, 1);
    asked(&world);
    world.issue(8, &["flow:feature"]);
    // The one slot goes to the consult of 7; 8 waits for it like any issue.
    let ticked = tick(&world, 1180);
    assert_eq!(did(&ticked), ["dispatch 7 done"]);
    assert_eq!((&ticked["held"], &ticked["frontier_remaining"]), (&json!(1), &json!(1)));
    assert!(started(&world).pop().unwrap().contains("AFKCONVOY_CONSULT='true'"));
    let next = tick(&world, 1270);
    assert_eq!(did(&next), [] as [&str; 0]);
    assert_eq!((&next["held"], &next["frontier_remaining"]), (&json!(1), &json!(1)));

    // Its ruling frees the slot, for the stage that asked.
    rule(&world, "Keep the v1 schema.");
    assert_eq!(did(&tick(&world, 1360)), ["advance 7 done", "dispatch 7 done"]);
}

#[test]
fn a_silent_consult_waits_for_the_operator_without_handing_its_claim_to_a_human() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    stop(&world, 1200);
    tick(&world, 2000);
    tick(&world, 3000);
    assert_eq!(world.claims().len(), 1);
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
    assert!(!world.comments(7).join("\n").contains("stopped with no ruling"));
    let plan = world.afk(&["plan", "--now", "3100"]).ok();
    assert_eq!(plan["host_kit"]["worker_health"]["workers"][0]["action"], "waiting for operator");
}

#[test]
fn a_verdict_declared_before_run_is_started_again_is_read_by_its_first_tick() {
    let world = bound(FEATURE, 2);
    declared_unread(&world);
    // `run` was killed before a tick read the verdict, and is started again:
    // the start takes the claim, and it is still the claim `build` declared under.
    let run = [
        "run",
        "--foreground",
        "--instance",
        "beta",
        "--host",
        "h1",
        "--second-ms",
        "0",
        "--ticks",
        "1",
        "--now",
    ];
    let (lines, _) = world.afk_lines(&[&run[..], &["2000"]].concat());
    assert!(lines.iter().any(|line| line.contains("took #7 stamped with this host")), "{lines:?}");
    let tick = lines.iter().find(|line| line.starts_with("tick 1:")).expect("a tick's line");
    assert!(tick.contains("advanced #7") && !tick.contains("nudged"), "{tick}");
    read_as_declared(&world);
}

#[test]
fn a_verdict_declared_before_a_takeover_by_hand_is_read_by_the_next_tick() {
    let world = bound(FEATURE, 2);
    declared_unread(&world);
    taken_over(&world, 1060);
    // Its worker, asked again, finds its verdict standing under the claim.
    let line = ["verdict", "escalate", "--issue", "7", "--stage", "build", "--text", "Again."];
    let again = world.afk(&[&line[..], &COMMITS].concat()).ok();
    assert_eq!((&again["declared"], &again["standing"]), (&json!(false), &json!("escalate")));
    assert_eq!(did(&tick_as(&world, "beta", 2000)), ["advance 7 done"]);
    read_as_declared(&world);
}

#[test]
fn a_verdict_declared_under_a_claim_gone_stale_is_read_by_the_tick_that_reclaims_it() {
    let world = bound(FEATURE, 2);
    declared_unread(&world);
    // The lease of `alpha` lapses, and a peer's tick takes its claim.
    assert_eq!(did(&tick_as(&world, "beta", 1000 + 4500 + 1)), ["reclaim 7 done"]);
    read_as_declared(&world);
}

#[test]
fn a_verdict_declared_after_its_claim_was_taken_over_is_read() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    taken_over(&world, 1060);
    declare(&world, "build", "escalate", "Which schema: v1 or v2?");
    stop(&world, 1070);
    assert_eq!(did(&tick_as(&world, "beta", 2000)), ["advance 7 done"]);
    read_as_declared(&world);
}

#[test]
fn a_ruling_declared_before_its_claim_is_taken_over_is_read() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    rule(&world, "Keep the v1 schema.");
    stop(&world, 1190);
    taken_over(&world, 1200);
    assert_eq!(did(&tick_as(&world, "beta", 3000)), ["advance 7 done", "dispatch 7 done"]);
    assert_eq!(world.claims(), [(7, "beta".to_string())]);
    let read = shown(&world);
    assert_eq!(
        (&read["position"]["is"], &read["position"]["stage"]),
        (&json!("perform"), &json!("build"))
    );
    assert_eq!(reads(&world), ["verdict", "verdict"]);
    // Nobody is told the consult stayed silent: it ruled.
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);
    assert!(!world.comments(7).join("\n").contains("stopped with no ruling"));
}

#[test]
fn a_taken_over_silent_stage_keeps_its_stage_and_claim_while_waiting_for_the_operator() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    stop(&world, 1010);
    taken_over(&world, 1060);
    tick_as(&world, "beta", 2000);
    tick_as(&world, "beta", 3000);
    assert_eq!(world.claims(), [(7, "beta".to_string())]);
    assert!(!world.comments(7).join("\n").contains("stopped with no ruling"));
    assert!(!with_a_human(&world));
}

/// An escalation is noticed and answered where a human looks (issue #149):
/// the tick that hands the issue over says so, the comment and the status
/// board end with the line that answers it, and `plan`, `doctor` and the
/// panel list it as waiting for a human — the same list.
#[test]
fn an_escalation_says_so_on_the_tick_line_and_names_the_command_that_answers_it() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    let_through(&world, "Not mine to rule: the schema is a product decision.");
    world.with_gh(|gh| gh["now"] = json!(1270));
    let run = [
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--ticks",
        "1",
        "--now",
        "1270",
    ];
    let (lines, _) = world.afk_lines(&[&run[..], &["--second-ms", "0"]].concat());
    assert_eq!(lines.len(), 1, "{lines:?}");
    let said = "tick 1: escalated #7 (build: consult let it through) · holding 0/2 · frontier 0";
    assert!(lines[0].starts_with(said) && !lines[0].contains("advanced"), "{lines:?}");

    // The comment and the status board end with each way out, the number filled in.
    let answer =
        format!("afkconvoy rule -C '{}' --issue 7 --text-file <file>", world.clone.display());
    let ways_out = format!(
        "**To answer it**, in a checkout of this repo:\n- rule on it: the stage `build` runs \
         again under your ruling — `{answer}`\n- close it: the fleet leaves a closed issue \
         alone — `gh issue close 7`"
    );
    let comments = world.comments(7);
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:account ")).count(),
        1,
        "an escalation leaves one work account: {comments:?}"
    );
    let comment = comments
        .into_iter()
        .find(|body| body.starts_with("<!--convoy:escalation "))
        .expect("an escalation comment");
    assert!(comment.trim_end().ends_with(&ways_out), "{comment}");
    let url = world.comment_url(7, &comment);
    world.comment(7, "A later status comment is not the reason for escalation.");
    let board = world.boards(7).pop().unwrap();
    assert!(board.contains("**Escalated to a human**") && board.ends_with(&ways_out), "{board}");

    // `plan` excludes it as escalated, not as an issue nobody labelled…
    let plan = world.afk(&["plan", "--now", "1300"]).ok();
    let excluded = plan["excluded"].as_array().unwrap();
    let reason = excluded.iter().find(|issue| issue["number"] == 7).map(|issue| &issue["reason"]);
    assert_eq!(reason, Some(&json!("escalated: waits for a ruling")), "{plan}");
    // …and lists it as waiting for a human, with the line that answers it.
    let why = "escalated at build: The consult of the stage `build` let it through.";
    let waiting = json!([{"issue": 7, "title": "issue 7", "why": why,
        "url": url, "since": 1270, "answer": answer}]);
    assert_eq!(plan["waiting_on_human"], waiting);

    // `doctor` gives the same list, and counts it a finding past the hour.
    let doctor = |now: &str| world.afk(&["doctor", "--host", "h1", "--now", now]).json;
    let waited = |answer: &Value| -> Vec<Value> {
        let findings = answer["findings"].as_array().unwrap().iter();
        let waits = |found: &&Value| found["what"].as_str().unwrap().contains("has waited");
        findings.filter(waits).cloned().collect()
    };
    let soon = doctor("4870");
    let mut doctor_waiting = waiting.clone();
    doctor_waiting[0]["taken_by"] = Value::Null;
    assert_eq!(soon["waiting_on_human"], doctor_waiting);
    assert_eq!(waited(&soon), [] as [Value; 0], "an hour is not more than an hour");
    let late = doctor("8571");
    assert_eq!(late["waiting_on_human"], doctor_waiting);
    let mut warnings = waited(&late);
    let identity = warnings[0].as_object_mut().unwrap().remove("watch_id").unwrap();
    let identity: Value =
        serde_json::from_str(identity.as_str().unwrap().strip_prefix("checkout:").unwrap())
            .unwrap();
    assert_eq!(identity[0], "human");
    let identity = identity[1].clone();
    let mut question = waiting[0].clone();
    question.as_object_mut().unwrap().remove("title");
    assert_eq!(identity, question, "the warning identifies the structured human question");
    assert_eq!(
        warnings,
        [json!({"kind": "waited_long", "severity": "warning",
            "what": format!("#7 has waited for a human for 2h 1m: {why}"),
            "evidence": "nobody holds it and no tick moves it; it is the fleet's again once \
                         it is answered, and it has waited longer than 1h 0m",
            "fix": answer, "see": "afkconvoy help humans"})]
    );

    // The panel's "With a human" is that list.
    let panel = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["panel", "--instance", "alpha", "--host", "h1", "--now", "1300"])
        .output()
        .unwrap();
    let page = String::from_utf8(panel.stdout).unwrap();
    let with_a_human = page.split("<h2>With a human</h2>").nth(1).expect("a list");
    let with_a_human = with_a_human.split("</section>").next().unwrap();
    assert_eq!(with_a_human.matches("<li class=\"card").count(), 1, "{with_a_human}");
    // labelled with its repo: the list is of every repo on the host
    let escaped = answer.replace('<', "&lt;").replace('>', "&gt;");
    let card = format!(
        "data-issue=\"7\"><span class=\"repo\">clone</span> <span class=\"num\">#7</span> \
         <span class=\"title\">issue 7</span><span class=\"meta\"><span \
         class=\"note\">escalated at build: The consult of the stage `build` let it through. — {escaped}</span>"
    );
    assert!(with_a_human.contains(&card), "{with_a_human}");
    assert!(with_a_human.contains(&format!("<a href=\"{url}\">Reason</a>")), "{with_a_human}");
    assert!(page.contains("With a human</dt><dd>1<"), "{page}");
    assert!(!page.contains("the fleet leaves alone"), "it is listed once: {page}");

    // The number is an argument or `--issue`, never both: the ruling answers it.
    let both = world.afk(&["rule", "7", "--issue", "7", "--text", "Keep the v1 schema."]);
    assert!(both.error().contains("cannot be used with"));
    assert_eq!(world.afk(&["show", "7"]).ok()["position"]["is"], "escalating");
    let ruling = world.clone.join("ruling.md");
    std::fs::write(&ruling, "Keep the v1 schema.\n").unwrap();
    let ruled = world.afk(&["rule", "7", "--text-file", ruling.to_str().unwrap()]).ok();
    assert_eq!((&ruled["ruled"], &ruled["stage"]), (&json!(true), &json!("build")), "{ruled}");
    assert!(world.labels(7).contains(&"flow:feature".to_string()));
    let plan = world.afk(&["plan", "--now", "1400"]).ok();
    assert_eq!(plan["waiting_on_human"], json!([]), "answered: it waits for nobody");
}

// Every hand-off goes to the consult first (docs/adr/0018): a failure the fleet
// saw, a verdict the stage may not declare, a breaker, a `blocked` that cannot
// clear, a changed workflow and an `already-satisfied` over work each start
// the stage's consult and not a human. Its ruling starts no count over, the
// same hand-off again goes to a human, and so does the consult's own failure.

/// Whether issue 7 is handed to a human: the escalate label in place of its
/// flow label.
fn with_a_human(world: &World) -> bool {
    let labels = world.labels(7);
    labels.contains(&"ready-for-human".to_string()) && !labels.contains(&"flow:feature".to_string())
}

/// What issue 7 is with, after the tick that read a hand-off: its consult,
/// which the next tick starts, briefed with what a human would have been
/// told — `told` among it — and no human.
fn consulted_on(world: &World, now: u64, told: &str) -> String {
    assert!(!with_a_human(world), "{:?}", world.labels(7));
    assert_eq!(shown(world)["position"]["is"], "consult");
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(world, now)), ["dispatch 7 done"]);
    assert!(started(world).pop().unwrap().contains("AFKCONVOY_CONSULT='true'"));
    let brief = brief(&worktree(world));
    for said in [
        "## What was asked\n\nThe fleet would have handed this issue to a human, and told them:",
        told,
    ] {
        assert!(brief.contains(said), "{said}\n{brief}");
    }
    brief
}

/// Issue 7's `build` dispatched at 1000, its worker silent at `stopped` and
/// past its nudge: the tick at `stopped + 1990` reads it.
fn silent(world: &World, stopped: u64) {
    stop(world, stopped);
    tick(world, stopped + 990);
    tick(world, stopped + 1990);
    assert!(!with_a_human(world));
}

#[test]
fn silent_stage_observations_do_not_consume_the_consult_or_breaker() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    silent(&world, 1010);
    assert_eq!(world.claims().len(), 1);
    assert!(!world.comments(7).join("\n").contains("stopped with no ruling"));
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
}

#[test]
fn a_verdict_its_stage_may_not_declare_goes_to_its_consult() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    // `build` has no `reject`.
    declare(&world, "build", "reject", "Not mine.");
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    consulted_on(&world, 1180, "> No valid verdict was declared at the stage `build`.");
    // The consult's own verdict that is none goes to a human.
    let line = ["verdict", "pass", "--issue", "7", "--stage", "build", "--text", "Fine."];
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(line)
        .env("AFKCONVOY_ROLE_COMMIT", "r1")
        .env("AFKCONVOY_TEAM_COMMIT", "t1")
        .env("AFKCONVOY_CONSULT", "true")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);
    assert!(with_a_human(&world), "{:?}", world.labels(7));
}

#[test]
fn a_tripped_breaker_goes_to_its_consult_and_one_tripping_again_to_a_human() {
    let world = bound(&FEATURE.replacen("```yaml\n", "```yaml\nmax_rework: 0\n", 1), 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, "build", "pass", "Built.");
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done", "dispatch 7 done"]);
    declare(&world, "review", "reject", "Not yet.");
    assert_eq!(did(&tick(&world, 1180)), ["advance 7 done"]);
    let brief = consulted_on(&world, 1270, "The breaker `max_rework` tripped");
    assert!(brief.contains("1. `review` declared `reject` — https://gh/c/"), "{brief}");
    rule(&world, "Ship it as it is.");
    assert_eq!(did(&tick(&world, 1360)), ["advance 7 done", "dispatch 7 done"]);
    declare(&world, "build", "pass", "Built again.");
    assert_eq!(did(&tick(&world, 1450)), ["advance 7 done", "dispatch 7 done"]);
    declare(&world, "review", "reject", "Still not.");
    assert_eq!(did(&tick(&world, 1540)), ["advance 7 done"]);
    assert!(with_a_human(&world), "{:?}", world.labels(7));
    let comments = world.comments(7);
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:account ")).count(),
        1,
        "an escalation leaves one work account: {comments:?}"
    );
    let comment = comments
        .into_iter()
        .find(|body| body.starts_with("<!--convoy:escalation"))
        .expect("an escalation comment");
    assert!(comment.contains("The breaker `max_rework` tripped"), "{comment}");
    let url = world.comment_url(7, &comment);
    world.comment(7, "Another status update.");
    for command in ["plan", "doctor"] {
        let view = world.afk(&[command, "--now", "1600"]).json;
        let waiting = &view["waiting_on_human"][0];
        assert!(
            waiting["why"].as_str().unwrap().contains("The breaker `max_rework` tripped:"),
            "{view}"
        );
        assert_eq!(waiting["url"], url);
    }
}

#[test]
fn a_blocked_whose_waiting_closes_a_cycle_goes_to_its_consult() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    world.with_gh(|gh| gh["edges"]["8"] = json!([7]));
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let line = ["verdict", "blocked", "--issue", "7", "--stage", "build", "--blocked-by", "8"];
    let declared = world.afk(&[&line[..], &["--text", "Needs #8."], &COMMITS].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    consulted_on(&world, 1180, "nothing will resolve what it waits on");
}

#[test]
fn a_workflow_changed_under_the_issue_goes_to_its_consult() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    // A record of `deploy`, a stage the workflow had once, with `build` to perform.
    world.comment(
        7,
        "<!--convoy:stage stage=deploy verdict=pass claim=c0 role_commit=r0 team_commit=t0-->\nold",
    );
    assert_eq!(shown(&world)["position"]["is"], "consult");
    consulted_on(&world, 1000, "> The workflow changed under this issue");
}

#[test]
fn already_satisfied_over_a_branch_that_holds_work_goes_to_its_consult() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    commit(&worktree(&world), "half.txt", "half\n");
    commit(&world.clone, "landed.txt", "only on the base branch\n");
    git(&world.clone, &["push", "-q", "origin", "main"]);
    declare(&world, "build", "already-satisfied", "Nothing to do.");
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    assert_eq!(world.state(7), "open");
    let evidence = world
        .comments(7)
        .into_iter()
        .find(|body| body.contains("half.txt"))
        .expect("the comment that names the work");
    assert!(!evidence.contains("landed.txt"), "{evidence}");
    consulted_on(&world, 1180, "already-satisfied");
}

#[test]
fn a_stage_that_writes_nothing_and_left_a_commit_goes_to_its_consult() {
    let review = "    writes: false\n    escalate: { to: reviewer, do: rule }\n";
    let world = bound(&FEATURE.replace("    writes: false\n", review), 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, "build", "pass", "Built.");
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done", "dispatch 7 done"]);
    commit(&worktree(&world), "fix.txt", "a fix while reviewing\n");
    declare(&world, "review", "pass", "Fine.");
    assert_eq!(did(&tick(&world, 1180)), ["advance 7 done"]);
    let told = consulted_on(&world, 1270, "is marked `writes: false`");
    assert!(told.contains("- **Stage that asked:** `review`"), "{told}");
}

#[test]
fn an_issue_handed_to_a_human_that_no_consult_saw_is_put_to_its_consult_once() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    // Handed over before every hand-off went to a consult first.
    let line = ["escalate", "--issue", "7", "--instance", "alpha", "--host", "h1"];
    world.afk(&[&line[..], &["--reason", "The gate stayed red: no space left."]].concat()).ok();
    assert!(with_a_human(&world), "{:?}", world.labels(7));
    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked), ["dispatch 7 done"]);
    assert!(!with_a_human(&world), "{:?}", world.labels(7));
    let told = brief(&worktree(&world));
    assert!(told.contains("> The gate stayed red: no space left."), "{told}");
    let records =
        world.comments(7).iter().filter(|c| c.contains("verdict=failed-escalated")).count();
    assert_eq!(records, 1);
    // Let through, it stays with the human: it is put once.
    let_through(&world, "A host problem, not mine to fix.");
    assert_eq!(did(&tick(&world, 1180)), ["advance 7 done"]);
    assert!(with_a_human(&world), "{:?}", world.labels(7));
    assert_eq!(did(&tick(&world, 1270)), [] as [&str; 0]);
    assert!(with_a_human(&world));
}

#[test]
fn a_failed_start_or_trust_gap_escalates_with_or_without_a_named_consult() {
    for named in [true, false] {
        let feature = if named {
            FEATURE.to_string()
        } else {
            FEATURE.replace("    escalate: { to: reviewer, do: rule }\n", "")
        };
        for knob in ["never_ready", "trust_prompt"] {
            let world = bound(&feature, 2);
            world.issue(7, &["flow:feature"]);
            world.with_orca(|orca| orca[knob] = json!(true));
            assert_eq!(tick(&world, 1000)["errors"].as_array().unwrap().len(), 1);
            if knob == "never_ready" {
                assert_eq!(world.claims().len(), 1, "the first failed start is continued");
                assert_eq!(tick(&world, 1090)["errors"].as_array().unwrap().len(), 1);
            }
            assert!(with_a_human(&world));
            assert_eq!(world.claims(), []);
            let comments = world.comments(7);
            let escalation =
                comments.iter().find(|body| body.contains("convoy:escalation")).unwrap();
            assert!(
                escalation.contains("Fleet fault:")
                    && escalation.contains("the fix is on the host")
            );
            // Fix the host: even with a working harness the next ticks must
            // leave the issue with its operator, taking no consult claim.
            world.with_orca(|orca| orca[knob] = json!(false));
            world.orca_calls();
            for now in [1180, 1270] {
                assert_eq!(did(&tick(&world, now)), [] as [&str; 0]);
                assert_eq!(world.claims(), []);
                assert!(with_a_human(&world));
            }
            assert_eq!(world.comments(7), comments, "no consult record is written");
            assert!(
                !world.orca_calls().iter().any(|call| common::asked(call) == "terminal create")
            );
            let doctor = world.afk(&["doctor", "--now", "1300"]).json;
            let why = doctor["waiting_on_human"][0]["why"].as_str().unwrap();
            assert!(
                why.contains("Fleet fault:") && why.contains("the fix is on the host"),
                "{doctor}"
            );
            // A human hands it back. Another failed start is a fleet fault
            // again, rather than a new opportunity to ask a consult.
            world.with_gh(|gh| gh["issues"][0]["labels"] = json!(["flow:feature"]));
            world.with_orca(|orca| orca[knob] = json!(true));
            tick(&world, 1360);
            if knob == "never_ready" {
                tick(&world, 1450);
            }
            assert_eq!(world.claims(), []);
            assert!(with_a_human(&world));
            let escalations =
                world.comments(7).iter().filter(|body| body.contains("convoy:escalation")).count();
            assert_eq!(escalations, 2);
            assert_eq!(did(&tick(&world, 1540)), [] as [&str; 0]);
        }
    }
}

#[test]
fn a_consult_runs_the_commands_a_human_answering_runs_and_the_two_lists_are_one() {
    use afkconvoy_core::tracker::Tracker;
    use afkconvoy_core::waiting::answering;
    let answering = answering(&Tracker::Own);
    // The commands the `humans` help names for answering an escalation.
    let help = world_help("humans");
    let section = help.split("\n## Answering an escalation\n").nth(1).expect("the section");
    let section = section.split("\n## ").next().unwrap();
    let named: Vec<&str> = section
        .lines()
        .filter_map(|line| line.strip_prefix("- `")?.split_once("`: "))
        .map(|(command, _)| command)
        .filter(|line| line.starts_with("afkconvoy ") || line.starts_with("gh "))
        .collect();
    assert_eq!(named, answering, "{section}");
    // The consult is given the same list, and its guard lets each through.
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    let told = brief(&worktree(&world));
    for line in &answering {
        let line = line.replace("<n>", "7");
        assert!(told.contains(&format!("    {line}\n")), "{line}\n{told}");
    }
    // A consult sends as a human does, under its claim.
    let line = ["send", "--issue", "7", "--stage", "review", "--text", "Built already; review it."];
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(line)
        .env("AFKCONVOY_ROLE_COMMIT", "r1")
        .env("AFKCONVOY_TEAM_COMMIT", "t1")
        .env("AFKCONVOY_CONSULT", "true")
        .output()
        .unwrap();
    let sent: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((&sent["declared"], &sent["verdict"]), (&json!(true), &json!("sent")), "{sent}");
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done", "dispatch 7 done"]);
    let read = shown(&world);
    assert_eq!(
        (&read["position"]["is"], &read["position"]["stage"]),
        (&json!("perform"), &json!("review"))
    );
    assert_eq!(read["rework"]["had"], 1, "an `escalate` it answers is rework, as by a ruling");
}

/// The text of `afkconvoy help <topic>`.
fn world_help(topic: &str) -> String {
    let world = World::new();
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["help", topic])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn consult_close_and_park_are_outcomes_and_a_park_waits_for_an_explicit_send() {
    for verdict in ["close", "park"] {
        let world = bound(FEATURE, 2);
        asked(&world);
        if verdict == "close" {
            commit(&worktree(&world), "investigation.txt", "builder evidence\n");
        }
        tick(&world, 1180);
        let text = "No reproduction after four investigations; reopen with raw output.";
        let args =
            ["verdict", verdict, "--consult", "--issue", "7", "--stage", "build", "--text", text];
        let declared = world.afk(&[&args[..], &COMMITS].concat()).ok();
        assert_eq!(declared["declared"], true);
        let read = shown(&world);
        assert_eq!(read["rework"]["had"], 0, "{read}");
        assert!(world.comments(7).last().unwrap().contains(text));
        assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);
        assert_eq!(world.claims(), []);
        assert_eq!(started(&world).len(), 2);
        assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);
        if verdict == "close" {
            assert!(worktree(&world).join("investigation.txt").exists());
            let plan = world.afk(&["plan", "--now", "1400"]).ok();
            assert_eq!(plan["frontier"], json!([]));
            assert_eq!(plan["waiting_on_human"], json!([]));
            continue;
        }
        assert_eq!(shown(&world)["position"]["is"], "parked-for-evidence");
        let parked = world.comments(7).pop().unwrap();
        let url = world.comment_url(7, &parked);
        world.comment(7, "A later status comment.");
        for command in ["plan", "doctor"] {
            let view = world.afk(&[command, "--now", "1400"]).ok();
            assert_eq!(view["waiting_on_human"][0]["issue"], 7, "{view}");
            assert!(view["waiting_on_human"][0]["why"].as_str().unwrap().contains("parked"));
            assert!(view["waiting_on_human"][0]["why"].as_str().unwrap().ends_with(text));
            assert_eq!(view["waiting_on_human"][0]["url"], url);
        }
        world.comment(7, "A routine status comment does not resume the investigation.");
        assert_eq!(did(&tick(&world, 1450)), [] as [&str; 0]);
        world
            .afk(&[
                "send",
                "--issue",
                "7",
                "--stage",
                "build",
                "--text",
                "New raw failure output attached.",
            ])
            .ok();
        assert_eq!(did(&tick(&world, 1540)), ["dispatch 7 done"]);
        assert!(brief(&worktree(&world)).contains("New raw failure output attached."));
    }
}

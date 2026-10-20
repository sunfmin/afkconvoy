//! A consult rules before a human is asked (docs/adr/0011): the `escalate` of
//! a stage that names one starts a worker in that role, which rules — and the
//! stage that asked runs again under the ruling — or lets the escalation
//! through to a human. It writes no code, and the chain is one role long.

use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
/// `build` names a consult; `review` names none.
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    model: opus
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
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", feature)]);
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
    assert_eq!(did(&tick(world, 1000)), ["dispatch 7 done"]);
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
    // The stage's model is its own worker's.
    assert!(commands[0].contains("--model 'opus'") && !consult.contains("--model"), "{consult}");

    let dir = worktree(&world);
    let told = brief(&dir);
    let own = format!("cd '{}' && {}", dir.display(), env!("CARGO_BIN_EXE_afkconvoy"));
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

    // A second tick over an unchanged world does nothing.
    assert_eq!(did(&tick(&world, 1270)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), 2);
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
        record.contains("the consult of the stage `build` ruled on its `escalate`"),
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
    assert!(told.contains("the consult of the stage `build` ruled on its `escalate`"), "{told}");
    assert!(told.contains("## Ruling\n\nThis stage declared `escalate`, and its consult ruled"));
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
fn a_stage_that_names_no_consult_escalates_to_a_human_as_before() {
    let world = bound(&FEATURE.replace("    escalate: { to: reviewer, do: rule }\n", ""), 2);
    asked(&world);
    let said = world.comments(7).join("\n");
    assert!(
        said.contains("The stage `build` declared `escalate`, and it names no consult."),
        "{said}"
    );
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1180)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), 1);
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
fn a_silent_consult_is_nudged_once_and_then_escalated() {
    let world = bound(FEATURE, 2);
    asked(&world);
    tick(&world, 1180);
    stop(&world, 1200);
    assert_eq!(did(&tick(&world, 2000)), ["advance 7 nudged"]);
    stop(&world, 2100);
    assert_eq!(did(&tick(&world, 3000)), ["advance 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(
        said.contains("The consult of the stage `build` stopped with no ruling and no `escalate`"),
        "{said}"
    );
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
}

#[test]
fn a_verdict_declared_before_run_is_started_again_is_read_by_its_first_tick() {
    let world = bound(FEATURE, 2);
    declared_unread(&world);
    // `run` was killed before a tick read the verdict, and is started again:
    // the start takes the claim, and it is still the claim `build` declared under.
    let run =
        ["run", "--instance", "beta", "--host", "h1", "--second-ms", "0", "--ticks", "1", "--now"];
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
fn a_silent_stage_whose_claim_was_taken_over_is_not_called_a_consult() {
    let world = bound(FEATURE, 2);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    stop(&world, 1010);
    taken_over(&world, 1060);
    assert_eq!(did(&tick_as(&world, "beta", 2000)), ["advance 7 nudged"]);
    stop(&world, 2100);
    assert_eq!(did(&tick_as(&world, "beta", 3000)), ["advance 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(
        said.contains("The worker of the stage `build` stopped with no valid verdict"),
        "{said}"
    );
    assert!(!said.contains("The consult of the stage"), "{said}");
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
    let run = ["run", "--instance", "alpha", "--host", "h1", "--ticks", "1", "--now", "1270"];
    let (lines, _) = world.afk_lines(&[&run[..], &["--second-ms", "0"]].concat());
    assert_eq!(lines.len(), 1, "{lines:?}");
    let said = "tick 1: escalated #7 (build: consult let it through) · holding 0/2 · frontier 0";
    assert!(lines[0].starts_with(said) && !lines[0].contains("advanced"), "{lines:?}");

    // The comment and the status board end with each way out, the number filled in.
    let answer = "afkconvoy rule --issue 7 --text-file <file>";
    let ways_out = format!(
        "**To answer it**, in a checkout of this repo:\n- rule on it: the stage `build` runs \
         again under your ruling — `{answer}`\n- close it: the fleet leaves a closed issue \
         alone — `gh issue close 7`"
    );
    let comment = world.comments(7).pop().unwrap();
    assert!(comment.starts_with("<!--convoy:escalation "), "{comment}");
    assert!(comment.trim_end().ends_with(&ways_out), "{comment}");
    let board = world.boards(7).pop().unwrap();
    assert!(board.contains("**Escalated to a human**") && board.ends_with(&ways_out), "{board}");

    // `plan` excludes it as escalated, not as an issue nobody labelled…
    let plan = world.afk(&["plan", "--now", "1300"]).ok();
    let excluded = plan["excluded"].as_array().unwrap();
    let reason = excluded.iter().find(|issue| issue["number"] == 7).map(|issue| &issue["reason"]);
    assert_eq!(reason, Some(&json!("escalated: waits for a ruling")), "{plan}");
    // …and lists it as waiting for a human, with the line that answers it.
    let waiting = json!([{"issue": 7, "title": "issue 7", "why": "escalated at build",
        "since": 1270, "answer": answer}]);
    assert_eq!(plan["waiting_on_human"], waiting);

    // `doctor` gives the same list, and counts it a finding past the hour.
    let doctor = |now: &str| world.afk(&["doctor", "--host", "h1", "--now", now]).json;
    let waited = |answer: &Value| -> Vec<Value> {
        let findings = answer["findings"].as_array().unwrap().iter();
        let waits = |found: &&Value| found["what"].as_str().unwrap().contains("has waited");
        findings.filter(waits).cloned().collect()
    };
    let soon = doctor("4870");
    assert_eq!(soon["waiting_on_human"], waiting);
    assert_eq!(waited(&soon), [] as [Value; 0], "an hour is not more than an hour");
    let late = doctor("8571");
    assert_eq!(late["waiting_on_human"], waiting);
    assert_eq!(
        waited(&late),
        [json!({"severity": "warning",
            "what": "#7 has waited for a human for 2h 1m: escalated at build",
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
    let card = "data-issue=\"7\"><span class=\"num\">#7</span> <span class=\"title\">issue 7\
                </span><span class=\"meta\"><span class=\"note\">escalated at build — afkconvoy \
                rule --issue 7 --text-file &lt;file&gt;</span>";
    assert!(with_a_human.contains(card), "{with_a_human}");
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

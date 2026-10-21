//! `rehearse` and `show`: a workflow walked through a list of verdicts with
//! no worker started, and drawn as a graph.

use crate::common;

use common::World;
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: plan
    by: dev
    do: plan
    escalate: { to: human }
    on: { pass: build }
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
    on: { pass: sign-off, reject: build }
  - name: sign-off
    by: human
    on: { pass: land, reject: plan }
land: { by: dev, do: land }
```
";
const TRIAGE: &str =
    "```yaml\nstages:\n  - { name: sort, by: reviewer, do: review, on: { pass: done } }\n```\n";

/// A world whose GitHub holds two roles and the team `acme/team`, of the
/// workflows `feature` and `triage`.
fn team() -> World {
    let world = World::new();
    for (role, skills) in
        [("acme/dev", &["plan", "implement", "land"][..]), ("acme/reviewer", &["review", "rule"])]
    {
        let files: Vec<String> =
            skills.iter().map(|skill| format!(".claude/skills/{skill}/SKILL.md")).collect();
        let files: Vec<(&str, &str)> = files.iter().map(|file| (file.as_str(), "a\n")).collect();
        world.repo(role, &files);
    }
    let workflows = [("workflows/feature.md", FEATURE), ("workflows/triage.md", TRIAGE)];
    world.repo("acme/team", &[&[("team.md", TEAM)][..], &workflows].concat());
    world
}

fn rehearse(world: &World, workflow: &str, declared: &[&str]) -> Value {
    let args = [&["rehearse", "--team", "acme/team", workflow][..], declared].concat();
    world.afk(&args).ok()
}

/// Each step of a rehearsal as "<stage> <declared> → <position>".
fn steps(rehearsed: &Value) -> Vec<String> {
    let steps = rehearsed["steps"].as_array().unwrap().iter();
    steps
        .map(|step| {
            let stage = step["stage"].as_str().unwrap_or("-");
            format!("{stage} {} → {}", step["declared"].as_str().unwrap(), at(&step["position"]))
        })
        .collect()
}

/// A position in a few words: what it is, at which stage, by whom.
fn at(position: &Value) -> String {
    let word = |key: &str| position[key].as_str().unwrap_or("-").to_string();
    format!("{} {} by {}: {}", word("is"), word("stage"), word("by"), word("do"))
}

/// The last step of a rehearsal.
fn last(rehearsed: &Value) -> &Value {
    rehearsed["steps"].as_array().unwrap().last().unwrap()
}

#[test]
fn each_verdict_is_answered_with_its_stage_and_the_position_that_follows() {
    let world = team();
    let rehearsed =
        rehearse(&world, "feature", &["pass", "pass", "reject", "pass", "pass", "reject", "pass"]);
    assert_eq!(rehearsed["workflow"], "feature");
    assert_eq!(at(&rehearsed["enters"]), "perform plan by dev: plan");
    assert_eq!(
        steps(&rehearsed),
        [
            "plan pass → perform build by dev: implement",
            "build pass → perform review by reviewer: review",
            "review reject → perform build by dev: implement",
            "build pass → perform review by reviewer: review",
            "review pass → waiting sign-off by human: -",
            "sign-off reject → perform plan by dev: plan",
            "plan pass → perform build by dev: implement",
        ]
    );
    assert_eq!(rehearsed["ends"], last(&rehearsed)["position"]);
    // A pass to `land` is the workflow's landing role's; one to `done` ends it.
    let landed = rehearse(&world, "feature", &["pass", "pass", "pass", "pass"]);
    assert_eq!(at(&landed["ends"]), "landing - by dev: land");
    assert_eq!(at(&rehearse(&world, "triage", &["pass"])["ends"]), "ended - by -: -");
    // Where a worker acts, the position says its harness: the default, in a
    // team and a config that name none. A human is a session of none.
    let harness = |rehearsed: &Value| rehearsed["ends"]["harness"].clone();
    assert_eq!(harness(&rehearse(&world, "feature", &[])), "claude-code");
    assert_eq!(harness(&rehearse(&world, "feature", &["pass", "escalate"])), "claude-code");
    assert_eq!(harness(&landed), "claude-code");
    assert_eq!(harness(&rehearse(&world, "feature", &["pass", "pass", "pass"])), Value::Null);
    assert_eq!(harness(&rehearse(&world, "triage", &["pass"])), Value::Null);
    // With nothing declared the issue is where it entered.
    let entered = rehearse(&world, "feature", &[]);
    assert_eq!((&entered["ends"], &entered["steps"]), (&entered["enters"], &json!([])));
    // Nothing was started and nothing written: GitHub was only asked for the
    // head of each repo, and cloned from.
    assert!(world.orca_calls().is_empty());
    let read =
        |call: &Vec<String>| call[..2] == ["repo", "clone"] || call[1].ends_with("/commits/HEAD");
    assert!(world.gh_calls().iter().all(read));
}

#[test]
fn blocked_parks_already_satisfied_ends_and_escalate_asks_the_consult_or_a_human() {
    let world = team();
    let parked = rehearse(&world, "feature", &["blocked", "pass"]);
    assert_eq!(
        steps(&parked),
        ["plan blocked → parked plan by -: -", "plan pass → perform build by dev: implement"]
    );
    assert_eq!(last(&parked)["rework"], 0, "running again after a park is no rework");
    assert_eq!(
        at(&rehearse(&world, "feature", &["pass", "already-satisfied"])["ends"]),
        "ended - by -: -"
    );
    // `build` names a consult, which rules or lets the escalation through.
    let ruled = rehearse(&world, "feature", &["pass", "escalate", "ruling"]);
    assert_eq!(
        steps(&ruled),
        [
            "plan pass → perform build by dev: implement",
            "build escalate → consult build by reviewer: rule",
            "build ruling → perform build by dev: implement",
        ]
    );
    assert_eq!(last(&ruled)["rework"], 1, "a consult's ruling is rework");
    let through = rehearse(&world, "feature", &["pass", "escalate", "escalate"]);
    assert_eq!(at(&through["ends"]), "escalating build by human: -");
    let reason = through["ends"]["reason"].as_str().unwrap();
    assert!(reason.contains("the consult of the stage `build` let"), "{reason}");
    // `plan` names none: a human is asked.
    let asked = rehearse(&world, "feature", &["escalate"]);
    assert_eq!(at(&asked["ends"]), "escalating plan by human: -");
    let reason = asked["ends"]["reason"].as_str().unwrap();
    assert!(reason.contains("`plan` declared `escalate`, which it hands to a human"), "{reason}");
}

#[test]
fn a_reject_from_a_stage_with_no_reject_is_no_valid_verdict() {
    let world = team();
    let rehearsed = rehearse(&world, "feature", &["pass", "reject"]);
    let step = last(&rehearsed);
    assert_eq!((&step["stage"], &step["valid"]), (&json!("build"), &json!(false)));
    // `build` names a consult: the hand-off goes to it before a human.
    assert_eq!(at(&step["position"]), "consult build by reviewer: rule");
    assert_eq!(step["position"]["reason"], "no valid verdict was declared at the stage `build`");
    let alone = rehearse(&world, "feature", &["reject"]);
    assert_eq!(at(&last(&alone)["position"]), "escalating plan by human: -");
    assert_eq!((&step["breaker"], &step["rework"]), (&Value::Null, &json!(0)));
    assert_eq!(rehearsed["steps"][0]["valid"], true);
    // Nothing is declared on an issue that has ended: it is in no stage.
    let ended = rehearse(&world, "triage", &["pass", "pass"]);
    let step = last(&ended);
    assert_eq!((&step["stage"], &step["valid"]), (&Value::Null, &json!(false)));
    assert_eq!(step["position"]["is"], "ended");
}

#[test]
fn the_rework_counts_are_answered_and_the_verdict_that_trips_a_breaker_is_marked() {
    let world = team();
    let counts = |step: &Value| {
        let in_a_row = &step["consecutive_rework"];
        (
            step["rework"].as_u64().unwrap(),
            in_a_row["review"].as_u64().unwrap(),
            in_a_row["sign-off"].as_u64().unwrap(),
        )
    };
    // `review` rejects three times in a row: max_consecutive_rework is 2.
    let twice = ["pass", "pass", "reject", "pass", "reject", "pass"];
    let rehearsed = rehearse(&world, "feature", &[&twice[..], &["reject"]].concat());
    assert_eq!(
        (rehearsed["max_rework"].as_u64(), rehearsed["max_consecutive_rework"].as_u64()),
        (Some(3), Some(2))
    );
    let all: Vec<_> = rehearsed["steps"].as_array().unwrap().iter().collect();
    assert_eq!(counts(all[2]), (1, 1, 0));
    assert_eq!(counts(all[4]), (2, 2, 0));
    assert!(all[..6].iter().all(|step| step["breaker"].is_null()));
    let tripped = all[6];
    assert_eq!(tripped["breaker"], "max_consecutive_rework");
    assert_eq!(counts(tripped), (2, 2, 0), "the rework a breaker refuses is not counted");
    assert_eq!(at(&tripped["position"]), "consult build by reviewer: rule");
    let reason = tripped["position"]["reason"].as_str().unwrap();
    assert!(reason.contains("the breaker `max_consecutive_rework` tripped"), "{reason}");
    // Its consult's ruling starts no count over, and the breaker that trips
    // again goes to a human.
    let again = [&twice[..], &["reject", "ruling", "pass", "reject"]].concat();
    let rehearsed = rehearse(&world, "feature", &again);
    let all: Vec<_> = rehearsed["steps"].as_array().unwrap().iter().collect();
    assert_eq!(counts(all[7]), (2, 2, 0));
    assert_eq!(at(&all[9]["position"]), "escalating build by human: -");
    // Only the consecutive counts of stages that may reject are answered.
    assert_eq!(tripped["consecutive_rework"], json!({"review": 2, "sign-off": 0}));

    // A fourth rework in total, no stage rejecting three times in a row.
    let three = [&twice[..], &["pass", "reject", "pass", "pass"]].concat();
    let rehearsed = rehearse(&world, "feature", &[&three[..], &["reject"]].concat());
    let all: Vec<_> = rehearsed["steps"].as_array().unwrap().iter().collect();
    // A pass of `review` started its count over, and `sign-off`'s rejection is its first.
    assert_eq!(counts(all[6]), (2, 0, 0));
    assert_eq!(counts(all[7]), (3, 0, 1));
    assert_eq!(all[10]["breaker"], "max_rework");
    assert_eq!(counts(all[10]), (3, 0, 1));
}

#[test]
fn a_human_ruling_starts_both_counts_over() {
    let world = team();
    // A breaker its consult let through, then a human's ruling.
    let tripped = ["pass", "pass", "reject", "pass", "reject", "pass", "reject", "escalate"];
    let rehearsed =
        rehearse(&world, "feature", &[&tripped[..], &["ruling", "pass", "reject"]].concat());
    let all: Vec<_> = rehearsed["steps"].as_array().unwrap().iter().collect();
    let ruled = all[8];
    assert_eq!((&ruled["stage"], &ruled["valid"]), (&json!("build"), &json!(true)));
    assert_eq!(at(&ruled["position"]), "perform build by dev: implement");
    assert_eq!(ruled["rework"], 0);
    assert_eq!(ruled["consecutive_rework"], json!({"review": 0, "sign-off": 0}));
    assert_eq!((&all[10]["rework"], &all[10]["breaker"]), (&json!(1), &Value::Null));
    // A ruling nobody asked for is no valid verdict.
    let stray = rehearse(&world, "feature", &["ruling"]);
    assert_eq!(last(&stray)["valid"], false);
}

#[test]
fn a_word_that_is_no_verdict_an_unknown_workflow_and_a_faulty_team_are_errors() {
    let world = team();
    let args = ["rehearse", "--team", "acme/team"];
    let error = world.afk(&[&args[..], &["feature", "pass", "fail"]].concat()).error();
    assert!(
        error.contains("pass, reject, blocked, already-satisfied, escalate, ruling"),
        "{error}"
    );
    let error = world.afk(&[&args[..], &["hotfix", "pass"]].concat()).error();
    assert_eq!(error, "the team acme/team has no workflow `hotfix`; it has: feature, triage");
    assert!(world.afk(&["show", "--team", "acme/team", "hotfix"]).error().contains("`hotfix`"));
    world.repo(
        "acme/broken",
        &[("team.md", TEAM), ("workflows/w.md", "```yaml\nstages: []\n```\n")],
    );
    for command in [&["rehearse", "w", "pass"][..], &["show"]] {
        let answer = world.afk(&[command, &["--team", "acme/broken"]].concat());
        assert_eq!(answer.code, 1);
        assert_eq!(answer.json["faults"][0]["key"], "stages", "{command:?}");
    }
    world.repo("acme/bare", &[("team.md", "```yaml\n{}\n```\n")]);
    let error = world.afk(&["show", "--team", "acme/bare"]).error();
    assert_eq!(error, "the team acme/bare has no workflow to draw");
}

/// What `show` prints for `args`: text, not JSON.
fn shown(world: &World, args: &[&str]) -> String {
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args([&["show", "--team", "acme/team"][..], args].concat())
        .output()
        .unwrap();
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn show_draws_every_stage_its_role_and_skill_and_every_pass_and_reject() {
    let world = team();
    let feature = "\
feature — flow:feature — max_rework 3, max_consecutive_rework 2

┌───▶ plan      dev: plan · harness claude-code · escalate → human
│       ↓ pass → build
│ ┌─▶ build     dev: implement · harness claude-code · escalate → reviewer: rule
│ │     ↓ pass → review
│ └── review    reviewer: review · harness claude-code · writes nothing · escalate → human · reject → build
│       ↓ pass → sign-off
└──── sign-off  human · reject → plan
        ↓ pass → land
      land      dev: land
";
    assert_eq!(shown(&world, &["feature"]), feature);
    let triage = "\
triage — flow:triage — max_rework 3, max_consecutive_rework 2

  sort  reviewer: review · harness claude-code
    ↓ pass → done
  done
";
    // With no name, every workflow of the team.
    assert_eq!(shown(&world, &[]), format!("{feature}\n{triage}"));
    // A drawing has no tab and no trailing space: it survives a paste into
    // a code block of a GitHub comment.
    assert!(!feature.contains('\t') && feature.lines().all(|line| line == line.trim_end()));
    assert!(world.orca_calls().is_empty());
}

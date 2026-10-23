//! A team's roster gives each role a range (docs/adr/0020): a stage's
//! worker runs its issue's casting within its role's range, a casting no
//! range of its workflow holds is not dispatched, the caster is given each
//! stage's range, and a range a role cannot run refuses the start.

use crate::common;

use afkconvoy_core::harness::Harness;
use common::{World, commit, git, write};
use serde_json::{Value, json};

const PLAN: [&str; 3] = ["plan", "--now", "1000"];
const TICK: [&str; 7] = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];

/// `dev` may run Claude Code on opus or pi on a/fast; `reviewer` runs
/// Claude Code on haiku alone.
const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\nroster:\n  \
                    dev:\n    cast: Small fixes on pi.\n    run:\n      - { harness: claude-code, \
                    model: opus }\n      - { harness: pi, model: a/fast, good_at: small fixes }\n  \
                    reviewer: { run: [{ harness: claude-code, model: haiku }] }\n```\n";
/// `build` by `dev`, then `review` by `reviewer`.
const FEATURE: &str = "\
```yaml
stages:
  - { name: build, by: dev, do: implement, on: { pass: review } }
  - { name: review, by: reviewer, do: review, writes: false, on: { pass: done } }
```
";
/// One stage, by `reviewer`.
const AUDIT: &str = "\
```yaml
stages:
  - { name: look, by: reviewer, do: review, writes: false, on: { pass: done } }
```
";

/// A repo bound to `team`, on a host kit that holds Claude Code on opus then
/// haiku, pi on a/fast then a/slow, and Codex and DSH on no model.
fn bound(team: &str) -> World {
    let world = World::new();
    world.hold_every_harness();
    world.kit_command("claude-code", Harness::ClaudeCode.stock());
    world.kit_models("claude-code", &["opus", "haiku"]);
    world.kit_models("pi", &["a/fast", "a/slow"]);
    let skill = |name: &str| {
        (
            format!(".claude/skills/{name}/SKILL.md"),
            format!("---\nname: {name}\ndescription: x\n---\n"),
        )
    };
    let (implement, review) = (skill("implement"), skill("review"));
    world.repo("acme/dev", &[(implement.0.as_str(), implement.1.as_str())]);
    world.repo("acme/reviewer", &[(review.0.as_str(), review.1.as_str())]);
    world.repo(
        "acme/team",
        &[("team.md", team), ("workflows/feature.md", FEATURE), ("workflows/audit.md", AUDIT)],
    );
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
                  concurrency: 9\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// What `plan` says of issue `number`: its `cast` on the frontier, or its
/// reason where it is excluded.
fn said(planned: &Value, number: u64) -> Value {
    let find = |key: &str| {
        planned[key].as_array().unwrap().iter().find(|issue| issue["number"] == number).cloned()
    };
    match (find("frontier"), find("excluded")) {
        (Some(issue), _) => issue["cast"].clone(),
        (None, Some(issue)) => issue["reason"].clone(),
        (None, None) => panic!("#{number} is nowhere: {planned}"),
    }
}

#[test]
fn a_stage_s_worker_runs_its_casting_within_its_role_s_range_and_plan_says_which_and_why() {
    let world = bound(TEAM);
    world.issue(7, &["flow:feature", "harness:pi", "model:a/fast"]);
    // `dev`'s range holds no haiku, `reviewer`'s does: its first.
    world.issue(8, &["flow:feature", "harness:claude-code", "model:haiku"]);
    // `reviewer`'s range holds one harness and one model: whatever the casting.
    world.issue(9, &["flow:audit", "harness:claude-code"]);
    world.issue(10, &["flow:audit"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(said(&planned, 7), json!({"harness": "pi", "model": "a/fast", "why": "casting"}));
    assert_eq!(
        said(&planned, 8),
        json!({"harness": "claude-code", "model": "opus", "why": "outside"})
    );
    for only in [9, 10] {
        assert_eq!(
            said(&planned, only),
            json!({"harness": "claude-code", "model": "haiku", "why": "only"})
        );
    }
    // Nothing has a choice to make but by its casting: no caster.
    assert_eq!(planned["caster"], Value::Null, "{planned}");
    world.afk(&TICK).ok();
    let orca = world.orca();
    let started: Vec<&str> = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|terminal| terminal["command"].as_str())
        .collect();
    assert_eq!(started.len(), 4, "{started:?}");
    assert_eq!(started.iter().filter(|line| line.contains(" && pi --approve ")).count(), 1);
    assert!(started.iter().any(|line| line.ends_with("--model 'a/fast'")), "{started:?}");
    assert_eq!(started.iter().filter(|line| line.ends_with("--model 'haiku'")).count(), 2);
}

#[test]
fn a_casting_no_range_of_its_workflow_holds_is_not_dispatched_and_plan_says_why() {
    let world = bound(TEAM);
    world.issue(7, &["flow:feature", "harness:codex"]);
    world.issue(8, &["flow:audit", "harness:pi", "model:a/fast"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        said(&planned, 7),
        "not dispatched: its casting `harness:codex` is held by no range of its workflow's \
         stages: build (dev): claude-code (opus); pi (a/fast) · review (reviewer): claude-code \
         (haiku)"
    );
    assert!(said(&planned, 8).as_str().unwrap().contains("look (reviewer): claude-code (haiku)"));
    world.afk(&TICK).ok();
    assert_eq!(world.worktrees().len(), 0, "nothing is started");
}

#[test]
fn the_caster_is_given_each_stage_s_range_and_casts_only_issues_with_a_choice_to_make() {
    let world = bound(TEAM);
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"]["to_cast"], json!([7]), "{planned}");
    let held = said(&planned, 7);
    assert!(held.as_str().unwrap().starts_with("held for the caster: "), "{planned}");
    assert_eq!(said(&planned, 8)["why"], "only");

    world.caster_answers(
        "{\"issue\": 7, \"harness\": \"pi\", \"model\": \"a/fast\", \"why\": \"A small fix.\"}\n",
    );
    let answered = world.afk(&["cast", "--now", "1000"]).ok();
    assert_eq!(
        answered["cast"],
        json!([{"issue": 7, "harness": "pi", "model": "a/fast", "why": "A small fix."}])
    );
    let asked = world.caster_asked();
    for given in [
        "- claude-code opus\n",
        "- pi a/fast: small fixes\n",
        "- claude-code haiku\n",
        "- dev: Small fixes on pi.\n",
        "Stage build (dev): claude-code (opus); pi (a/fast)\n",
        "Stage review (reviewer): claude-code (haiku)\n",
    ] {
        assert!(asked.contains(given), "{given:?}\n{asked}");
    }
    assert!(!asked.contains("## #8") && !asked.contains("codex"), "{asked}");
    assert_eq!(world.labels(7), ["flow:feature", "harness:pi", "model:a/fast"]);
}

#[test]
fn a_role_whose_range_is_empty_or_holds_a_harness_it_has_no_limits_for_refuses_the_start() {
    // `reviewer` may run a model the host kit does not hold.
    let world = bound(&TEAM.replace("model: haiku", "model: sonnet"));
    world.issue(7, &["flow:feature"]);
    for start in [&PLAN[..], &TICK] {
        let error = world.afk(start).error();
        assert!(
            error.contains(
                "the role `reviewer` has an empty range on this host: the roster allows it \
                 claude-code (sonnet), and the host kit holds claude-code (opus, haiku); pi \
                 (a/fast, a/slow); codex (no model); dsh (no model)"
            ),
            "{error}"
        );
    }
    assert_eq!(world.worktrees().len(), 0);

    // A role the roster names none for ranges over the host kit whole: one
    // that holds limits for Claude Code alone cannot run pi.
    let world =
        bound(&TEAM.replace("  reviewer: { run: [{ harness: claude-code, model: haiku }] }\n", ""));
    let reviewer = world.root.join("github/acme/reviewer");
    commit(&reviewer, ".claude/settings.json", r#"{"hooks": {"PreToolUse": [{"hooks": []}]}}"#);
    world.issue(7, &["flow:feature"]);
    let error = world.afk(&PLAN).error();
    assert!(
        error.contains(
            "the role `reviewer` may run `pi`, and holds limits for `claude-code` (in its \
             `.claude/settings.json`) and none for `pi`"
        ),
        "{error}"
    );
    assert!(error.contains("the roster allows it the host kit whole"), "{error}");
}

//! A team's roster gives each role a range (docs/adr/0020): a stage's worker
//! runs its one `model:` label of the selected harness, or that harness's
//! user default, within the role's range. A label the range does not hold is
//! not followed. Automatic casting is gone. A role that can run nothing
//! safely only waits.

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
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.list_checkout(&world.clone);
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
fn a_missing_label_is_the_user_default_and_a_model_switch_does_not_choose_it() {
    let team = TEAM
        .replace("model: opus }", "model: opus }\n      - { harness: claude-code, model: haiku }");
    let world = bound(&team);
    let other = world.second_repo();
    world.issue(7, &["flow:feature"]);
    let planned = world.afk(&PLAN).ok();
    assert!(planned["host_kit"].get("cast").is_none(), "{planned}");
    assert!(planned.get("caster").is_none(), "{planned}");
    assert_eq!(said(&planned, 7)["model"], Value::Null);
    assert_eq!(said(&planned, 7)["why"], "user_default");
    assert_eq!(planned["casting"][0]["why"], "the user's default");
    // Moving a model first inside its harness does not name the worker's model.
    world.afk(&["switch", "--model", "haiku"]).ok();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(said(&planned, 7)["model"], Value::Null);
    assert_eq!(said(&planned, 7)["why"], "user_default");
    let mut gh = world.gh2();
    gh["issues"] =
        json!([{"number":8,"title":"task", "labels":["ready-for-agent"], "state":"open"}]);
    std::fs::write(world.root.join("gh2.json"), gh.to_string()).unwrap();
    let unbound = world.afk_in(&other, &PLAN).ok()["frontier"][0]["cast"].clone();
    assert_eq!(unbound["model"], Value::Null);
    assert_eq!(unbound["why"], "user_default");
    world.afk(&TICK).ok();
    world.afk_in(&other, &TICK).ok();
    let terminals = world.orca()["terminals"].as_array().unwrap().clone();
    assert_eq!(terminals.len(), 2);
    assert!(terminals.iter().all(|t| !t["command"].as_str().unwrap().contains("--model")));
    let planned = world.afk(&PLAN).ok();
    assert!(planned["casting"][0]["running"]["model"].is_null(), "{planned}");
    let doctor = world.afk(&["doctor", "--now", "1000"]).json;
    assert!(doctor["issues"]["casting"][0]["running"]["model"].is_null(), "{doctor}");
    // One model label of the selected harness is followed, harness label or not.
    world.issue(9, &["flow:feature", "model:opus"]);
    let followed = said(&world.afk(&PLAN).ok(), 9);
    assert_eq!(followed["model"], "opus");
    assert_eq!(followed["why"], "model_label");
}

#[test]
fn a_harness_with_no_models_runs_its_user_default_and_cast_is_gone() {
    let world = bound("```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n");
    assert!(world.afk(&["kit", "--cast", "on"]).error().contains("automatic casting is gone"));
    world.afk(&["switch", "--harness", "codex"]).ok();
    world.issue(7, &["flow:feature"]);
    let planned = world.afk(&PLAN).ok();
    assert!(planned.get("caster").is_none(), "{planned}");
    assert_eq!(said(&planned, 7)["harness"], "codex");
    assert_eq!(said(&planned, 7)["model"], Value::Null);
    assert_eq!(said(&planned, 7)["why"], "user_default");
    assert_eq!(planned["casting"][0]["why"], "the user's default");
    world.afk(&TICK).ok();
    assert_eq!(world.worktrees().len(), 1);
    assert!(world.caster_asked().is_empty());
}

#[test]
fn bound_and_unbound_checkouts_share_kit_priority_while_role_limits_remain_constraints() {
    let world = bound(TEAM);
    let other = world.second_repo();
    world.issue(7, &["flow:feature", "harness:claude-code", "model:opus"]);
    world.issue(9, &["flow:audit", "harness:pi", "model:a/fast"]);
    let mut gh = world.gh2();
    gh["issues"] = json!([{"number":8,"title":"unbound task", "labels":["ready-for-agent","harness:claude-code","model:opus"], "state":"open"}]);
    std::fs::write(world.root.join("gh2.json"), gh.to_string()).unwrap();
    world.afk_in(&world.root, &["switch", "--harness", "pi"]).ok();
    let bound_plan = world.afk(&PLAN).ok();
    let unbound_plan = world.afk_in(&other, &PLAN).ok();
    // `model:opus` names nothing pi holds, so the worker runs pi's user default.
    assert_eq!(said(&bound_plan, 7)["harness"], "pi");
    assert_eq!(said(&bound_plan, 7)["model"], Value::Null);
    assert_eq!(said(&bound_plan, 7)["why"], "user_default");
    assert_eq!(said(&bound_plan, 7)["unmatched"], "`model:opus`");
    let unbound = &unbound_plan["frontier"][0]["cast"];
    assert_eq!(unbound["harness"], "pi");
    assert_eq!(unbound["model"], Value::Null);
    assert_eq!(unbound["why"], "user_default");
    // The audit role allows Claude Code alone. A pi label is not followed there.
    assert_eq!(said(&bound_plan, 9)["harness"], "claude-code");
    assert_eq!(said(&bound_plan, 9)["model"], Value::Null);
    assert_eq!(said(&bound_plan, 9)["why"], "user_default");
    assert_eq!(said(&bound_plan, 9)["unmatched"], "`model:a/fast`");
}

#[test]
fn a_stage_s_worker_runs_its_casting_within_its_role_s_range_and_plan_says_which_and_why() {
    let world = bound(TEAM);
    // `a/fast` is a pi model. The selected harness is Claude Code, so it is not followed.
    world.issue(7, &["flow:feature", "harness:pi", "model:a/fast"]);
    // `dev`'s range holds no haiku, though the kit's Claude Code does.
    world.issue(8, &["flow:feature", "harness:claude-code", "model:haiku"]);
    // No model label, including a range that holds one model: the user default.
    world.issue(9, &["flow:audit", "harness:claude-code"]);
    world.issue(10, &["flow:audit"]);
    let planned = world.afk(&PLAN).ok();
    let seven = said(&planned, 7);
    assert_eq!(seven["harness"], "claude-code");
    assert_eq!(seven["model"], Value::Null);
    assert_eq!(seven["why"], "user_default");
    assert_eq!(seven["unmatched"], "`model:a/fast`");
    let eight = said(&planned, 8);
    assert_eq!(eight["harness"], "claude-code");
    assert_eq!(eight["model"], Value::Null);
    assert_eq!(eight["why"], "user_default");
    assert_eq!(eight["unmatched"], "`model:haiku`");
    assert!(eight["notice"].as_str().unwrap().contains("does not hold"), "{eight}");
    for only in [9, 10] {
        let cast = said(&planned, only);
        assert_eq!(cast["harness"], "claude-code");
        assert_eq!(cast["model"], Value::Null);
        assert_eq!(cast["why"], "user_default");
        assert!(cast.get("unmatched").is_none(), "{cast}");
    }
    assert!(planned.get("caster").is_none(), "{planned}");
    world.afk(&TICK).ok();
    let orca = world.orca();
    let started: Vec<&str> = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|terminal| terminal["command"].as_str())
        .collect();
    assert_eq!(started.len(), 4, "{started:?}");
    assert!(started.iter().all(|line| !line.contains("--model")), "{started:?}");
}

#[test]
fn casting_outside_the_selected_harness_is_preserved_without_overriding_kit_priority() {
    let world = bound(TEAM);
    world.issue(7, &["flow:feature", "harness:codex"]);
    world.issue(8, &["flow:audit", "harness:pi", "model:a/fast"]);
    world.issue(9, &["flow:feature", "harness:claude-code", "model:sonnet"]);
    let planned = world.afk(&PLAN).ok();
    let seven = said(&planned, 7);
    assert_eq!(seven["harness"], "claude-code");
    assert_eq!(seven["model"], Value::Null);
    assert_eq!(seven["why"], "user_default");
    assert!(seven.get("unmatched").is_none(), "a harness label is not a model label: {seven}");
    let nine = said(&planned, 9);
    assert_eq!(nine["harness"], "claude-code");
    assert_eq!(nine["model"], Value::Null);
    assert_eq!(nine["why"], "user_default");
    assert_eq!(nine["unmatched"], "`model:sonnet`");
    let eight = said(&planned, 8);
    assert_eq!(eight["harness"], "claude-code");
    assert_eq!(eight["model"], Value::Null);
    assert_eq!(eight["unmatched"], "`model:a/fast`");
    world.afk(&TICK).ok();
    assert_eq!(world.worktrees().len(), 3);
    assert!(world.labels(7).contains(&"harness:codex".to_string()));
    assert!(world.labels(8).contains(&"harness:pi".to_string()));
    assert!(world.labels(9).contains(&"model:sonnet".to_string()));
    assert!(!world.comments(7).iter().any(|c| c.starts_with("<!--convoy:unmatched-->")));
    assert!(world.comments(9).iter().any(|c| c.contains("The label stays.")));
}
#[test]
fn no_label_runs_the_user_default_and_cast_starts_nothing() {
    let world =
        bound(&TEAM.replace(
            "model: opus }",
            "model: opus }\n      - { harness: claude-code, model: haiku }",
        ));
    assert!(world.afk(&["kit", "--cast", "on"]).error().contains("automatic casting is gone"));
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    let planned = world.afk(&PLAN).ok();
    assert!(planned.get("caster").is_none(), "{planned}");
    for number in [7, 8] {
        let cast = said(&planned, number);
        assert_eq!(cast["harness"], "claude-code", "{cast}");
        assert_eq!(cast["model"], Value::Null, "one model in a range is still the user default");
        assert_eq!(cast["why"], "user_default");
    }
    assert!(world.afk(&["cast", "--now", "1000"]).error().contains("automatic casting is gone"));
    assert_eq!(world.labels(7), ["flow:feature"]);
    assert_eq!(world.labels(8), ["flow:audit"]);
    assert!(world.caster_asked().is_empty());
}

#[test]
fn a_roster_entry_that_matches_nothing_ranges_over_the_host_kit_and_is_named() {
    // `reviewer` may run a model the host kit does not hold.
    let world = bound(&TEAM.replace("model: haiku", "model: sonnet"));
    world.issue(7, &["flow:audit"]);
    let checked = world.afk(&["check"]).ok();
    let reviewer = &checked["roles"][1];
    assert_eq!(
        reviewer["range"][0],
        json!({"harness": "claude-code", "models": ["opus", "haiku"]})
    );
    let unmatched = reviewer["unmatched"].as_str().unwrap();
    assert!(
        unmatched.starts_with(
            "the roster's entry for the role `reviewer` allows it claude-code (sonnet), none of \
             which the host kit holds"
        ),
        "{unmatched}"
    );
    assert_eq!(checked["roles"][0]["unmatched"], Value::Null);
    // `plan` and `tick` start, and say so.
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["unmatched"][0]["role"], "reviewer", "{planned}");
    let ticked = world.afk(&TICK).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
}

#[test]
fn a_role_with_nothing_it_can_run_safely_waits_and_goes_on_once_it_has() {
    // `reviewer` holds limits for Claude Code alone, and Claude Code is
    // paused: nothing the host kit holds is safe for it.
    let world = bound("```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n");
    let hooked = r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": []}]}}"#;
    commit(&world.root.join("github/acme/reviewer"), ".claude/settings.json", hooked);
    world.pause("claude-code", 900, "refused every prompt");
    world.issue(7, &["flow:feature", "harness:pi", "model:a/fast"]);
    world.issue(8, &["flow:audit", "harness:claude-code", "model:haiku"]);

    let planned = world.afk(&PLAN).ok();
    assert_eq!(said(&planned, 7)["harness"], "pi", "{planned}");
    let waits = said(&planned, 8);
    assert!(waits.as_str().unwrap().contains("no currently eligible harness"), "{planned}");
    assert_eq!(planned["nothing_safe"][0]["role"], "reviewer", "{planned}");
    let ticked = world.afk(&TICK).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert_eq!(world.worktrees().len(), 1, "only dev's stage is started");
    let doctor = world.afk(&["doctor"]).json;
    assert!(doctor.to_string().contains("reviewer has no harness on this host"), "{doctor}");

    // Claude Code back: its stage is dispatched, with no one doing anything.
    std::fs::remove_file(world.paused_path()).unwrap();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        said(&planned, 8),
        json!({"harness": "claude-code", "model": "haiku", "why": "model_label"}),
        "{planned}"
    );
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1100"]).ok();
    assert_eq!(world.worktrees().len(), 2);
    // No worker ran a harness its role holds no limits for.
    let orca = world.orca();
    let reviewer_on_pi = orca["terminals"].as_array().unwrap().iter().any(|terminal| {
        let line = terminal["command"].as_str().unwrap_or_default();
        line.contains("reviewer") && !line.contains("claude")
    });
    assert!(!reviewer_on_pi, "{orca}");
}

#[test]
fn a_role_s_range_leaves_out_a_harness_it_holds_no_limits_for_and_plan_and_check_say_so() {
    // No roster: every role ranges over the host kit, less what it holds no
    // limits for. `reviewer` holds limits for Claude Code and Codex alone.
    let world = bound("```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n");
    let reviewer = world.root.join("github/acme/reviewer");
    commit(&reviewer, ".claude/settings.json", r#"{"hooks": {"PreToolUse": [{"hooks": []}]}}"#);
    commit(&reviewer, ".codex/hooks.json", r#"{"hooks": {"PreToolUse": [{"hooks": []}]}}"#);
    world.issue(7, &["flow:feature"]);

    let checked = world.afk(&["check"]).ok();
    let reviewer = &checked["roles"][1];
    assert_eq!(reviewer["limits"], json!(["claude-code", "codex"]));
    assert_eq!(
        reviewer["range"],
        json!([
            {"harness": "claude-code", "models": ["opus", "haiku"]},
            {"harness": "codex", "models": []},
        ])
    );
    let left_out = json!([
        {"harness": "pi", "why": "no limits for pi"},
        {"harness": "dsh", "why": "no limits for dsh"},
        {"harness": "kimi", "why": "no limits for kimi"},
        {"harness": "grok", "why": "no limits for grok"},
    ]);
    assert_eq!(reviewer["left_out"], left_out);
    // A role with no limits at all is right for every harness.
    assert_eq!(checked["roles"][0]["left_out"], json!([]));

    // `plan` and `run` start, and say the same.
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["left_out"],
        json!([{"role": "dev", "left_out": []}, {"role": "reviewer", "left_out": left_out}])
    );
    assert!(world.afk(&["kit", "--cast", "on"]).error().contains("automatic casting is gone"));
    let ticked = world.afk(&TICK).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(!ticked["did"].to_string().contains("\"cast\""), "{ticked}");
    assert_eq!(world.worktrees().len(), 1, "the issue is dispatched, not held");
}

#[test]
fn a_team_s_roster_is_drafted_from_the_host_kit_into_its_working_tree_and_committed_by_nobody() {
    let world = bound("```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n");
    // `reviewer` holds limits for Claude Code alone.
    let hooked = r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": []}]}}"#;
    commit(&world.root.join("github/acme/reviewer"), ".claude/settings.json", hooked);
    world.with_kit(|kit| {
        for h in kit["harnesses"].as_array_mut().unwrap() {
            let Some(models) = h.get_mut("models").and_then(Value::as_array_mut) else { continue };
            for m in models {
                m["good_at"] = match m["model"].as_str().unwrap() {
                    "opus" => json!("hard, wide work"),
                    "haiku" => json!("reviews"),
                    "a/fast" => json!("small fixes"),
                    _ => json!("not researched"),
                };
            }
        }
    });
    let team = world.root.join("github/acme/team");
    let dir = team.to_str().unwrap();
    let head = git(&team, &["rev-parse", "HEAD"]);
    let draft = ["draft", "--dir", dir, "--team", "acme/team"];
    let answer = world.afk(&draft);
    let drafted = answer.json.clone();
    assert_eq!(answer.code, 0, "{drafted}");
    assert_eq!(drafted["written"], true);
    assert_eq!(drafted["replaces"], false);
    assert!(drafted.get("drafted_by").is_none(), "{drafted}");
    assert_eq!(
        drafted["unsaid"],
        json!([
            "what codex is good at",
            "what dsh is good at",
            "what kimi is good at",
            "what grok is good at"
        ])
    );
    assert!(answer.stderr.contains("the draft says nothing of what codex is good at"));
    assert!(world.drafter_asked().is_empty(), "a draft sends no prompt");
    assert!(!world.shell_asked().iter().any(|line| line.contains("You are")));

    // Written into the working tree, and nothing committed. A model whose
    // good_at is `not researched` is not placed, so the draft leaves it out.
    // No `cast` line is written.
    let written = std::fs::read_to_string(team.join("team.md")).unwrap();
    let roster = concat!(
        "roster:\n",
        "  dev:\n",
        "    run:\n",
        "      - { harness: claude-code, model: \"opus\", good_at: \"hard, wide work\" }\n",
        "      - { harness: claude-code, model: \"haiku\", good_at: \"reviews\" }\n",
        "      - { harness: pi, model: \"a/fast\", good_at: \"small fixes\" }\n",
        "      - { harness: codex }\n",
        "      - { harness: dsh }\n",
        "      - { harness: kimi }\n",
        "      - { harness: grok }\n",
        "  reviewer:\n",
        "    run:\n",
        "      - { harness: claude-code, model: \"opus\", good_at: \"hard, wide work\" }\n",
        "      - { harness: claude-code, model: \"haiku\", good_at: \"reviews\" }\n",
        "    # left out: pi, codex, dsh, kimi, grok, which the role holds no limits for\n",
    );
    assert_eq!(
        written,
        format!("```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n{roster}```\n")
    );
    assert_eq!(drafted["roster"], roster);
    assert_eq!(git(&team, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&team, &["status", "--porcelain"]).trim(), "M team.md");

    // The draft passes `check` as written.
    let checked = world.afk(&["check", "--dir", dir, "--team", "acme/team"]).ok();
    assert_eq!(checked["roles"][0]["roster"]["run"].as_array().unwrap().len(), 7, "{checked}");
    assert_eq!(checked["roles"][1]["roster"]["run"].as_array().unwrap().len(), 2, "{checked}");

    // Where a roster is, it says what it would change and writes nothing
    // unconfirmed; `--yes` confirms. A changed host kit supplies the new line.
    assert_eq!(world.afk(&draft).ok()["written"], false);
    world.with_kit(|kit| kit["harnesses"][1]["models"][0]["good_at"] = json!("one-line fixes"));

    let asked_again = world.afk(&draft);
    let unconfirmed = asked_again.json.clone();
    assert_eq!(asked_again.code, 0, "{unconfirmed}");
    assert_eq!((&unconfirmed["replaces"], &unconfirmed["written"]), (&json!(true), &json!(false)));
    let line = "      - { harness: pi, model: \"a/fast\", good_at: \"small fixes\" }";
    assert_eq!(unconfirmed["removed"], json!([line]));
    assert_eq!(unconfirmed["added"], json!([line.replace("small fixes", "one-line fixes")]));
    assert!(asked_again.stderr.contains(&format!("- {line}\n")), "{}", asked_again.stderr);
    assert!(asked_again.stderr.contains("nothing written"), "{}", asked_again.stderr);
    assert_eq!(std::fs::read_to_string(team.join("team.md")).unwrap(), written);
    let confirmed = world.afk(&[&draft[..], &["--yes"]].concat()).ok();
    assert_eq!(confirmed["written"], true);
    assert!(std::fs::read_to_string(team.join("team.md")).unwrap().contains("one-line fixes"));
    // The same draft again changes nothing, and writes nothing.
    let same = world.afk(&draft).ok();
    assert_eq!(
        (&same["written"], &same["removed"], &same["added"]),
        (&json!(false), &json!([]), &json!([]))
    );
}

/// `dev` names Claude Code's opus by its display name and pi's a/fast without
/// its provider; `reviewer` names a model the host kit holds by no name.
const NAMED: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\nroster:\n  \
                     dev:\n    run:\n      - { harness: claude-code, model: Opus 5.5 }\n      \
                     - { harness: pi, model: fast }\n  reviewer: { run: [{ harness: \
                     claude-code, model: opuss }, { harness: claude-code, model: haiku }] }\n```\n";

#[test]
fn a_roster_names_a_model_by_any_name_and_one_naming_nothing_is_said_and_opened_once_in_the_team_s_repo()
 {
    let world = bound(NAMED);
    world.with_kit(|kit| {
        let claude = &mut kit["harnesses"][0];
        claude["models"][0] = json!({"model": "claude-opus-5-5", "names": ["opus", "Opus 5.5"]});
    });
    world.issue(7, &["flow:feature", "harness:claude-code", "model:opus"]);
    // `check` gives each range by ids, and names the entry that matches nothing.
    let checked = world.afk(&["check"]).ok();
    let team = &checked;
    let dev = team["roles"].as_array().unwrap().iter().find(|r| r["name"] == "dev").unwrap();
    assert_eq!(
        dev["range"],
        json!([{"harness": "claude-code", "models": ["claude-opus-5-5"]},
            {"harness": "pi", "models": ["a/fast"]}]),
        "{checked}"
    );
    let unmatched = &team["unmatched_models"];
    assert_eq!(unmatched.as_array().unwrap().len(), 1, "{checked}");
    assert_eq!(
        (&unmatched[0]["role"], &unmatched[0]["model"]),
        (&json!("reviewer"), &json!("opuss"))
    );
    assert!(unmatched[0]["said"].as_str().unwrap().contains("the nearest names: `opus`"));
    // `plan` names it, and casts by any name, running the id.
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["unmatched_models"][0]["model"], "opuss", "{planned}");
    assert_eq!(
        said(&planned, 7),
        json!({"harness": "claude-code", "model": "claude-opus-5-5", "why": "model_label"})
    );
    // A tick opens one issue in the team's repo, with its ready label; the
    // next opens none while it is open.
    world.afk(&TICK).ok();
    world.afk(&TICK).ok();
    let gh = world.gh();
    let opened = gh["other_issues"]["acme/team"].as_array().unwrap();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0]["labels"], json!(["ready-for-agent"]));
    let body = opened[0]["body"].as_str().unwrap();
    for named in
        ["`reviewer`", "opuss", "the nearest names: `opus`", "claude-code (claude-opus-5-5, haiku)"]
    {
        assert!(body.contains(named), "{named}: {body}");
    }
    // `doctor` names it too.
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    let found = doctor["findings"].as_array().unwrap();
    assert!(
        found
            .iter()
            .any(|f| f["what"].as_str().unwrap().contains("names no model the host kit holds")),
        "{doctor}"
    );
}

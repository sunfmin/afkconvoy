use crate::common;

use afkconvoy_core::harness::Harness;
use common::World;
use serde_json::{Value, json};

const TICK: [&str; 7] = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
const PLAN: [&str; 3] = ["plan", "--now", "1000"];

/// The command each worker's terminal was started with.
fn started(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    terminals.iter().map(|t| t["command"].as_str().unwrap().to_string()).collect()
}

/// An unbound repo whose issue 7 carries `labels` beside the ready label, on
/// a host kit that holds Claude Code on `opus` then `haiku`, pi on `a/fast`
/// then `a/slow`, and Codex and DSH on no model.
fn cast(labels: &[&str]) -> World {
    let world = World::new();
    world.hold_every_harness();
    world.issue(7, &[&["ready-for-agent"], labels].concat());
    let mut kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    let models = |names: &[&str]| -> Value {
        names
            .iter()
            .map(|name| json!({"model": name, "probed": {"at": 900, "seconds": 1.0}}))
            .collect()
    };
    for held in kit["harnesses"].as_array_mut().unwrap() {
        held["models"] = match held["harness"].as_str().unwrap() {
            "claude-code" => models(&["opus", "haiku"]),
            "pi" => models(&["a/fast", "a/slow"]),
            _ => json!([]),
        };
    }
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    world
}

#[test]
fn an_issue_s_casting_names_the_harness_and_model_its_worker_runs() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "pi", "model": "a/slow", "why": "casting"})
    );
    let ticked = world.afk(&TICK).ok();
    assert_eq!(
        started(&world),
        [Harness::Pi.with_model(Harness::Pi.stock(), "a/slow")],
        "{ticked}"
    );
    // A harness named alone is started on its command as written.
    let world = cast(&["harness:codex"]);
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
}

#[test]
fn an_issue_with_no_casting_and_no_choice_to_make_runs_the_host_kit_s_first() {
    // The host kit holds one harness on one model: there is nothing to cast.
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    world.with_kit(|kit| kit["harnesses"][0]["models"] = json!([{"model": "opus"}]));
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "claude-code", "model": "opus", "why": "first"})
    );
    assert_eq!(planned["caster"], Value::Null);
    world.afk(&TICK).ok();
    assert_eq!(started(&world), ["agent --model 'opus'"]);
}

#[test]
fn a_casting_the_host_kit_does_not_hold_runs_its_first_and_the_issue_is_told_once() {
    for (labels, name) in [
        (&["harness:pi", "model:a/none"][..], "`model:a/none`"),
        (&["harness:codex", "model:gpt-x"][..], "`model:gpt-x`"),
        (&["harness:nope"][..], "`harness:nope`"),
        (&["model:opus"][..], "`model:opus`"),
    ] {
        let world = cast(labels);
        let planned = world.afk(&PLAN).ok();
        assert_eq!(planned["excluded"], json!([]), "{labels:?}");
        assert_eq!(
            planned["frontier"][0]["cast"],
            json!({"harness": "claude-code", "model": "opus", "why": "unmatched", "unmatched": name}),
            "{labels:?}"
        );
        world.afk(&TICK).ok();
        let started = started(&world);
        assert_eq!(started.len(), 1, "{labels:?}");
        assert!(started[0].ends_with("--model 'opus'"), "{started:?}");
        let told = |world: &World| -> Vec<String> {
            let comments = world.comments(7);
            comments
                .into_iter()
                .filter(|body| body.starts_with("<!--convoy:unmatched-->"))
                .collect()
        };
        let said = told(&world);
        assert_eq!(said.len(), 1, "{said:?}");
        for part in [
            &format!("{name} matches nothing this host holds"),
            "this worker runs `claude-code` on `opus` instead",
            "The host kit holds claude-code (opus, haiku); pi (a/fast, a/slow)",
        ] {
            assert!(said[0].contains(part), "{part}\n{}", said[0]);
        }
        if labels == ["harness:pi", "model:a/none"] {
            assert!(said[0].contains("the nearest names:"), "{}", said[0]);
        }
        // The label is never taken off, and the next tick tells nothing again.
        assert!(world.labels(7).iter().any(|label| label == labels[0]), "{labels:?}");
        world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1100"]).ok();
        assert_eq!(told(&world).len(), 1, "{labels:?}");
    }
}

#[test]
fn a_switch_wins_over_the_casting() {
    let world = cast(&["harness:pi", "model:a/none"]);
    let switch = ["switch", "--instance", "alpha", "--host", "h1", "--now", "900"];
    world.afk(&[&switch[..], &["--all", "--harness", "codex"]].concat()).ok();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "codex", "model": null, "why": "switch"})
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
}

/// Ticks as `alpha` at `now`.
fn tick_at(world: &World, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

/// The terminals started on a caster: each runs `afkconvoy cast`.
fn casters(world: &World) -> usize {
    started(world).iter().filter(|command| command.contains(" cast; exit")).count()
}

#[test]
fn a_tick_starts_one_caster_and_holds_the_issues_it_casts_back_for_ten_minutes_at_most() {
    let world = cast(&[]);
    world.issue(8, &["ready-for-agent"]);
    // A human's casting is never the caster's: 9 is dispatched as it says.
    world.issue(9, &["ready-for-agent", "harness:codex"]);
    let ticked = tick_at(&world, 1000);
    assert_eq!(casters(&world), 1, "{ticked}");
    assert_eq!(started(&world).len(), 2, "the caster and the worker on 9: {ticked}");
    assert!(started(&world).contains(&Harness::Codex.stock().to_string()));
    assert_eq!(ticked["did"][0], json!({"do": "cast", "issue": null, "outcome": "done"}));

    let planned = world.afk(&["plan", "--now", "1100"]).ok();
    assert_eq!(
        planned["caster"],
        json!({"running_since": 1000, "due": false, "held": [7, 8], "to_cast": [7, 8]})
    );
    for excluded in &planned["excluded"].as_array().unwrap()[..2] {
        let reason = excluded["reason"].as_str().unwrap();
        assert!(reason.starts_with("held for the caster"), "{excluded}");
    }
    let panel = world.render_panel("caster-runs", &["--now", "1100"], (1440, 1000)).html;
    assert!(panel.contains("<b>A caster runs</b>"), "{panel}");
    assert!(panel.contains("#7, #8 held for it"), "{panel}");

    // No second caster while it runs, and nothing it is to cast is dispatched.
    tick_at(&world, 1300);
    assert_eq!((casters(&world), started(&world).len()), (1, 2));

    // Past ten minutes they are dispatched uncast, on the host kit's first.
    let planned = world.afk(&["plan", "--now", "1600"]).ok();
    assert_eq!(planned["caster"], Value::Null);
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "claude-code", "model": "opus", "why": "uncast"})
    );
    let panel = world.render_panel("caster-late", &["--now", "1600"], (1440, 1000)).html;
    assert!(panel.contains("the caster did not cast it in ten minutes"), "{panel}");
    tick_at(&world, 1600);
    let opus = started(&world).iter().filter(|c| *c == "agent --model 'opus'").count();
    assert_eq!((casters(&world), opus), (1, 2));
}

#[test]
fn an_issue_with_no_choice_to_make_or_a_casting_starts_no_caster() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick_at(&world, 1000);
    assert_eq!((casters(&world), started(&world)), (0, vec!["agent".to_string()]));

    let world = cast(&["harness:pi"]);
    tick_at(&world, 1000);
    assert_eq!(casters(&world), 0);
}

#[test]
fn cast_labels_each_issue_within_the_host_kit_and_comments_why() {
    let world = cast(&[]);
    world.issue(8, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent", "harness:pi"]);
    world.issue(10, &["ready-for-agent", "epic"]);
    world.issue(11, &["ready-for-agent"]);
    world.with_kit(|kit| kit["harnesses"][1]["models"][0]["good_at"] = json!("quick, small fixes"));
    world.caster_answers(concat!(
        "Here you are:\n",
        "{\"issue\": 7, \"harness\": \"pi\", \"model\": \"a/fast\", \"why\": \"A one-line fix.\"}\n",
        "{\"issue\": 8, \"harness\": \"codex\", \"model\": null, \"why\": \"A wide refactor.\"}\n",
        "{\"issue\": 9, \"harness\": \"claude-code\", \"model\": \"opus\", \"why\": \"Not asked.\"}\n",
        "{\"issue\": 11, \"harness\": \"pi\", \"model\": \"a/none\", \"why\": \"Out of range.\"}\n",
    ));
    let answered = world.afk(&["cast", "--now", "1000"]).ok();
    assert_eq!(
        answered["cast"],
        json!([
            {"issue": 7, "harness": "pi", "model": "a/fast", "why": "A one-line fix."},
            {"issue": 8, "harness": "codex", "model": null, "why": "A wide refactor."},
        ])
    );
    assert_eq!(answered["uncast"][0]["issue"], 11, "{answered}");
    assert!(answered["uncast"][0]["why"].as_str().unwrap().contains("`model:a/none`"));

    // The host kit's first was asked, once, of every issue that is to be cast.
    let asked = world.caster_asked();
    for given in ["## #7 issue 7", "## #8 issue 8", "## #11 issue 11"] {
        assert!(asked.contains(given), "{given}\n{asked}");
    }
    assert!(!asked.contains("## #9") && !asked.contains("## #10"), "{asked}");
    assert!(asked.contains("- pi a/fast: quick, small fixes\n"), "{asked}");
    assert!(asked.contains("- codex\n") && asked.contains("- claude-code opus\n"), "{asked}");
    assert!(
        world.shell_asked().iter().any(|line| line.starts_with("agent --model 'opus' --print"))
    );

    let gh = world.gh();
    let labels = |number: u64| {
        let issue = gh["issues"].as_array().unwrap().iter().find(|i| i["number"] == number);
        issue.unwrap()["labels"].clone()
    };
    assert_eq!(labels(7), json!(["ready-for-agent", "harness:pi", "model:a/fast"]));
    assert_eq!(labels(8), json!(["ready-for-agent", "harness:codex"]));
    assert_eq!(labels(9), json!(["ready-for-agent", "harness:pi"]), "a human's is left as it is");
    assert_eq!(labels(11), json!(["ready-for-agent"]));
    assert_eq!(
        gh["comments"]["7"][0]["body"],
        "Cast `harness:pi` and `model:a/fast`: A one-line fix."
    );
    assert_eq!(gh["comments"]["9"], Value::Null);
}

#[test]
fn a_switch_cleared_lets_the_casting_stand_again_and_moves_the_worker_it_names_onto_it() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let switch = ["switch", "--instance", "alpha", "--host", "h1", "--now", "900"];
    world.afk(&[&switch[..], &["--harness", "codex"]].concat()).ok();
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);

    let cleared = world.afk(&[&switch[..], &["7", "--clear"]].concat()).ok();
    assert_eq!(cleared["chosen"], json!({"was": "codex", "now": null}));
    assert_eq!((&cleared["from"], &cleared["to"]), (&json!("codex"), &json!("pi")), "{cleared}");
    let pi = Harness::Pi.with_model(Harness::Pi.stock(), "a/slow");
    assert_eq!(started(&world).last(), Some(&pi));

    // the next worker of the repo runs its casting
    world.issue(8, &["ready-for-agent", "harness:pi", "model:a/fast"]);
    let planned = world.afk(&PLAN).ok();
    let eight = planned["frontier"].as_array().unwrap().iter().find(|issue| issue["number"] == 8);
    assert_eq!(
        eight.expect("issue 8 on the frontier")["cast"],
        json!({"harness": "pi", "model": "a/fast", "why": "casting"})
    );
}

#[test]
fn a_paused_harness_is_left_out_of_the_host_kit_and_its_issue_waits_with_plan_saying_why() {
    // A human cast 7 to pi; pi is paused on this host.
    let world = cast(&["harness:pi", "model:a/slow"]);
    let kit = std::fs::read(world.kit_path()).unwrap();
    world.pause("pi", 400, "organization disabled subscription access");
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"], json!([]), "{planned}");
    let reason = planned["excluded"][0]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("held while its harness is paused: `pi` is paused on this host since ")
            && reason.contains("organization disabled subscription access"),
        "{reason}"
    );
    let paused = &planned["host_kit"]["paused"][0];
    assert_eq!((&paused["harness"], &paused["since"]), (&json!("pi"), &json!(400)), "{planned}");
    assert_eq!(paused["why"], "organization disabled subscription access");
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [] as [String; 0], "no worker on a paused harness");
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:pi", "model:a/slow"]);

    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    let found =
        doctor["findings"].as_array().unwrap().iter().find(|finding| {
            finding["what"].as_str().unwrap().contains("`pi` is paused on this host")
        });
    let found = found.unwrap_or_else(|| panic!("{doctor}"));
    assert!(found["evidence"].as_str().unwrap().contains("since 1970-01-01 00:06 UTC"), "{found}");
    let panel = world.render_panel("paused", &["--now", "1000"], (1440, 1000)).html;
    assert!(panel.contains("<b>The harness pi is paused on this host</b>"), "{panel}");
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit, "the host kit's file is unchanged");
}

#[test]
fn a_host_kit_every_harness_of_which_is_paused_starts_no_worker_and_plan_says_so() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.pause("claude-code", 400, "refused");
    let planned = world.afk(&PLAN).ok();
    let reason = planned["excluded"][0]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("held while every harness of the host kit is paused: `claude-code`"),
        "{reason}"
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [] as [String; 0]);
}

#[test]
fn an_issue_the_caster_cast_to_a_paused_harness_is_cast_again_and_it_says_why() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.comment(7, "Cast `harness:pi` and `model:a/slow`: A small fix.");
    // A human's casting to it waits; one a worker started on waits too.
    world.issue(8, &["ready-for-agent", "harness:pi"]);
    world.pause("pi", 400, "refused");

    let ticked = tick_at(&world, 1000);
    assert_eq!(casters(&world), 1, "{ticked}");
    let planned = world.afk(&["plan", "--now", "1100"]).ok();
    assert_eq!(planned["caster"]["to_cast"], json!([7]), "{planned}");
    assert_eq!(planned["caster"]["held"], json!([7]), "{planned}");
    let eight = planned["excluded"].as_array().unwrap().iter().find(|e| e["number"] == 8);
    assert!(eight.unwrap()["reason"].as_str().unwrap().starts_with("held while its harness"));

    world.caster_answers(
        "{\"issue\": 7, \"harness\": \"codex\", \"model\": null, \"why\": \"pi is out.\"}\n",
    );
    let answered = world.afk(&["cast", "--now", "1100"]).ok();
    assert_eq!(answered["cast"][0]["harness"], "codex", "{answered}");
    assert!(!world.caster_asked().contains("- pi "), "a paused harness is offered to no caster");
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:codex"]);
    let said = world.comments(7).last().unwrap().clone();
    assert!(said.starts_with("Cast `harness:codex`: pi is out."), "{said}");
    assert!(
        said.contains("Cast again, in place of `harness:pi` and `model:a/slow`: `pi` is paused"),
        "{said}"
    );
    assert_eq!(world.labels(8), ["ready-for-agent", "harness:pi"], "a human's is not changed");
}

#[test]
fn each_paused_harness_is_probed_every_hour_and_one_that_answers_is_back() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.pause("pi", 0, "refused");
    tick_at(&world, 3599);
    assert_eq!(world.paused()[0]["probed"], Value::Null, "not probed within the hour");
    tick_at(&world, 3600);
    assert_eq!(world.paused()[0]["probed"], 3600, "probed, and silent");
    // In an orca terminal of its own, closed once it was silent.
    let questions = world.questions();
    let probes: Vec<_> =
        questions.iter().filter(|(asked, _)| asked.contains("single word OK")).collect();
    assert_eq!(probes.len(), 1, "{questions:?}");
    assert!(!probes[0].1, "its terminal is closed");
    assert_eq!(started(&world), [] as [String; 0]);

    world.probes_answer();
    tick_at(&world, 7199);
    assert_eq!(world.paused().as_array().unwrap().len(), 1, "an hour after its last probe");
    tick_at(&world, 7200);
    assert_eq!(world.paused(), json!([]), "it answered");
    assert_eq!(started(&world), [Harness::Pi.with_model(Harness::Pi.stock(), "a/slow")]);
}

#[test]
fn a_caster_s_casting_naming_no_held_model_is_cast_again_with_ids_and_a_human_s_stays() {
    let world = cast(&["harness:claude-code", "model:sonnet"]);
    world.comment(7, "Cast `harness:claude-code` and `model:sonnet`: A small fix.");
    // A human's label naming nothing stays and runs the default.
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:opuss"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"]["to_cast"], json!([7]), "{planned}");
    let eight = planned["frontier"].as_array().unwrap().iter().find(|e| e["number"] == 8).unwrap();
    assert_eq!(eight["cast"]["why"], "unmatched");
    assert_eq!(eight["cast"]["unmatched"], "`model:opuss`");

    // The caster answers with a name; the label names the model's id.
    world.with_kit(|kit| {
        kit["harnesses"][0]["models"][0] = json!({"model": "claude-opus-5-5", "names": ["opus"]});
    });
    world.caster_answers(
        "{\"issue\": 7, \"harness\": \"claude-code\", \"model\": \"OPUS\", \"why\": \"Hard.\"}\n",
    );
    let answered = world.afk(&["cast", "--now", "1100"]).ok();
    assert_eq!(answered["cast"][0]["model"], "claude-opus-5-5", "{answered}");
    assert_eq!(
        world.labels(7),
        ["ready-for-agent", "harness:claude-code", "model:claude-opus-5-5"]
    );
    let said = world.comments(7).last().unwrap().clone();
    assert!(said.starts_with("Cast `harness:claude-code` and `model:claude-opus-5-5`: Hard."));
    assert!(said.contains("Cast again, in place of `harness:claude-code` and `model:sonnet`: its label `model:sonnet`"), "{said}");
    assert_eq!(world.labels(8), ["ready-for-agent", "harness:claude-code", "model:opuss"]);
}

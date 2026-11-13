//! A human rules, steers and may hold a stage (docs/adr/0010, docs/adr/0011):
//! `rule` answers an escalated issue and its stage runs again, `send` puts an
//! issue at a named stage, and a stage a workflow gives to a human waits for
//! their verdict with no claim and no slot held.

use crate::common;

use std::path::PathBuf;

use common::{World, git, write};
use serde_json::{Value, json};

/// The team names its own escalate label: a bound repo's config says none.
const TEAM: &str =
    "```yaml\nescalate_label: stuck\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
max_rework: 1
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: human }
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    escalate: { to: human }
    on: { pass: sign-off, reject: build }
  - name: sign-off
    by: human
    on: { pass: done, reject: build }
```
";

/// A world whose repo is bound to a team with the one workflow `feature`,
///.
fn bound() -> World {
    bound_to(FEATURE)
}

/// `bound`, the workflow `feature` being `feature`.
fn bound_to(feature: &str) -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    world.repo(
        "acme/reviewer",
        &[(".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n")],
    );
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", feature)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
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
        !["board", "stage_label", "heartbeat"].contains(&step["do"].as_str().unwrap())
            && step["outcome"] != "nothing"
    });
    kept.map(|step| {
        let word = |key: &str| step[key].as_str().unwrap();
        format!("{} {} {}", word("do"), step["issue"], word("outcome"))
    })
    .collect()
}

/// Whether a worker holds `issue`.
fn held(world: &World, issue: u64) -> bool {
    world.claims().iter().any(|(claimed, _)| *claimed == issue)
}

/// One whole stage of `issue`, from `now`: a tick starts its worker, unless
/// the tick that carried the last verdict out did; the worker declares
/// `verdict`, and the next tick carries that out — and starts the worker of
/// the stage it leads to, when a worker performs one.
fn stage(world: &World, issue: u64, now: u64, stage: &str, verdict: &str) {
    if !held(world, issue) {
        assert_eq!(did(&tick(world, now)), [format!("dispatch {issue} done")], "{stage}");
    }
    let number = issue.to_string();
    let line = ["verdict", verdict, "--issue", &number, "--stage", stage, "--text", "As asked."];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    assert_eq!(world.afk(&[&line[..], &commits].concat()).ok()["declared"], true);
    let carried = did(&tick(world, now + 30));
    let mut expected = vec![format!("advance {issue} done")];
    if held(world, issue) {
        expected.push(format!("dispatch {issue} done"));
    }
    assert_eq!(carried, expected, "{stage}");
}

/// Nobody holds `issue` any more: the claim its worker was started under is
/// released by hand.
fn let_go(world: &World, issue: u64) {
    let issue = issue.to_string();
    world.afk(&["release", "--issue", &issue, "--instance", "alpha", "--host", "h1"]).ok();
}

fn show(world: &World, issue: u64) -> Value {
    world.afk(&["show", "--issue", &issue.to_string()]).ok()
}

/// Where `show` says `issue` is: what its position is, and at which stage.
fn at(world: &World, issue: u64) -> (String, String) {
    let position = &show(world, issue)["position"];
    let word = |key: &str| position[key].as_str().unwrap_or_default().to_string();
    (word("is"), word("stage"))
}

fn frontier(world: &World) -> Vec<(u64, String)> {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let rows = plan["frontier"].as_array().unwrap().iter();
    rows.map(|row| (row["number"].as_u64().unwrap(), row["stage"].as_str().unwrap().to_string()))
        .collect()
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

/// The stage records on `issue`, oldest first, as their comments stand.
fn records(world: &World, issue: u64) -> Vec<String> {
    let comments = world.comments(issue).into_iter();
    comments.filter(|body| body.starts_with("<!--convoy:stage ")).collect()
}

fn has(world: &World, issue: u64, label: &str) -> bool {
    world.labels(issue).contains(&label.to_string())
}

/// An escalation transferred to a tracker before the repo was bound. Its
/// old comment has neither a workflow nor a current command to answer it.
fn escalation_with_no_position() -> World {
    let feature = FEATURE.replace("max_rework: 1", "label: ready-for-agent\nmax_rework: 1");
    let world = bound_to(&feature);
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, "acme/tracker");
    world.with_tracker(|gh| {
        gh["issues"] = json!([{"number": 11, "title": "moved issue", "state": "open",
            "labels": ["repo:acme/dev", "stuck"]}]);
        gh["comments"]["11"] = json!([{"id": 1, "at": 1000, "html_url": "https://gh/c/1",
            "body": "<!--convoy:escalation claim=old-->\nFix the failure."}]);
    });
    world
}

/// Every view gives the same recovery, including the refused commands:
/// nobody can rule on a stage or send to one before choosing a workflow.
fn recovery_with_no_position(world: &World, answer: &str, choices: &[&str]) {
    let before = world.tracker();
    let show = world.afk(&["show", "--issue", "11"]).error();
    assert!(show.contains("issue #11 has no position"), "{show}");
    let plan = world.afk(&["plan", "--now", "5000"]).ok();
    let waiting = &plan["waiting_on_human"][0];
    assert_eq!(waiting["answer"], answer, "{plan}");
    let why = waiting["why"].as_str().unwrap();
    assert!(why.ends_with(": Fix the failure."), "{waiting}");
    assert_eq!(waiting["url"], "https://gh/c/1");
    for label in choices {
        assert!(why.contains(label), "the choice {label} is beside the answer: {waiting}");
    }
    let doctor = world.afk(&["doctor", "--now", "5000"]).json;
    assert_eq!(doctor["waiting_on_human"], plan["waiting_on_human"]);
    let findings = doctor["findings"].as_array().unwrap();
    let waited = findings.iter().find(|f| f["what"].as_str().unwrap().contains("has waited"));
    let waited = waited.expect("waiting more than an hour is a finding");
    assert_eq!(waited["fix"], answer);
    for label in choices {
        assert!(waited["what"].as_str().unwrap().contains(label), "{waited}");
    }

    let panel = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["panel", "--now", "5000"])
        .output()
        .unwrap();
    assert!(panel.status.success(), "{}", String::from_utf8_lossy(&panel.stderr));
    let page = String::from_utf8(panel.stdout).unwrap();
    let escaped = answer.replace('<', "&lt;").replace('>', "&gt;");
    assert!(page.contains(&escaped), "{page}");
    for label in choices {
        assert!(page.contains(label), "{page}");
    }
    for command in [vec!["rule"], vec!["send", "--stage", "build"]] {
        let error =
            world.afk(&[&command[..], &["--issue", "11", "--text", "Continue."]].concat()).error();
        assert!(error.contains(answer), "{error}");
        for label in choices {
            assert!(error.contains(label), "{error}");
        }
    }
    let after = world.tracker();
    for key in ["issues", "comments"] {
        assert_eq!(before[key], after[key], "no refused command or view writes {key}");
    }
}

#[test]
fn an_escalation_with_no_position_names_the_only_flow_label_in_its_answer() {
    let world = escalation_with_no_position();
    let answer =
        "gh issue edit 11 --repo acme/tracker --add-label ready-for-agent --remove-label stuck";
    recovery_with_no_position(&world, answer, &[]);
}

#[test]
fn an_escalation_with_no_position_lists_each_flow_label_to_choose_from() {
    let world = escalation_with_no_position();
    let team = world.root.join("github/acme/team");
    let chore = FEATURE.replace("max_rework: 1", "label: needs triage\nmax_rework: 1");
    write(&team, "workflows/chore.md", &chore);
    write(&team, "workflows/fix.md", FEATURE);
    git(&team, &["add", "."]);
    git(&team, &["commit", "-q", "-m", "add workflows"]);
    let answer =
        "gh issue edit 11 --repo acme/tracker --add-label '<flow label>' --remove-label stuck";
    recovery_with_no_position(&world, answer, &["ready-for-agent", "needs triage", "flow:fix"]);
}

#[test]
fn a_ruling_runs_the_stage_that_asked_again_with_the_ruling_in_its_history() {
    let world = bound();
    world.issue(7, &["flow:feature", "bug"]);
    stage(&world, 7, 1000, "build", "escalate");
    // Handed to a human under the label the team names, with no flow label.
    assert_eq!(world.labels(7), ["bug", "stage:build", "stuck"]);
    let said = world.comments(7).join("\n");
    assert!(said.contains("<!--convoy:escalation claim="), "{said}");
    assert!(said.contains(" flow=feature-->"), "it says which workflow it left: {said}");
    // It still has a position: the one it was escalated from.
    assert_eq!(at(&world, 7), ("escalating".to_string(), "build".to_string()));
    assert_eq!(frontier(&world), []);

    let ruling = "Keep the v1 schema.\nMigrate in a later issue.";
    let ruled = world.afk(&["rule", "--issue", "7", "--text", ruling]).ok();
    assert_eq!(
        (&ruled["stage"], &ruled["ruled"], &ruled["relabelled"], &ruled["workflow"]),
        (&json!("build"), &json!(true), &json!(true), &json!("feature"))
    );
    let record = records(&world, 7).pop().unwrap();
    assert!(
        record.starts_with(
            "<!--convoy:stage stage=build verdict=ruling by=human-->\n**afkconvoy** — a human \
             ruled on the stage `build`"
        ),
        "{record}"
    );
    assert!(record.ends_with(ruling), "{record}");
    // The escalate label is swapped back for the flow label, and nothing else moved.
    assert_eq!(world.labels(7), ["bug", "stage:build", "flow:feature"]);
    assert_eq!(world.claims(), []);

    // A human's record is told apart from a worker's.
    let read = show(&world, 7);
    let left = read["records"].as_array().unwrap();
    assert_eq!((&left[0]["by"], &left[0]["role_commit"]), (&json!("worker"), &json!("r1")));
    let last = left.last().unwrap();
    assert_eq!(
        (&last["by"], &last["verdict"], &last["reads"], &last["claim"], &last["role_commit"]),
        (&json!("human"), &json!("ruling"), &json!("verdict"), &Value::Null, &Value::Null)
    );
    assert_eq!(read["position"]["is"], "perform");

    // The next tick runs the stage that asked, under the ruling.
    assert_eq!(frontier(&world), [(7, "build".to_string())]);
    assert_eq!(did(&tick(&world, 1090)), ["dispatch 7 done"]);
    let told = brief(&world, 7);
    assert!(told.contains("- **Stage:** `build`, of the workflow `feature`\n"), "{told}");
    assert!(told.contains("- `build` declared `escalate` — https://gh/c/"), "{told}");
    assert!(told.contains("- a human ruled on the stage `build`"), "{told}");
    assert!(
        told.contains("## Ruling\n") && told.contains("> Keep the v1 schema.\n> Migrate in a"),
        "{told}"
    );
    // A claim is on it now: it is its worker's, and a second ruling is refused.
    let held = world.afk(&["rule", "--issue", "7", "--text", "And another."]).error();
    assert!(held.contains("issue #7 is claimed"), "{held}");
    assert_eq!(records(&world, 7).pop().unwrap(), record, "nothing was written");
}

#[test]
fn a_workflow_that_names_the_ready_label_takes_an_issue_opened_with_it_and_a_ruling_puts_it_back() {
    let named = FEATURE.replace("```yaml\n", "```yaml\nlabel: ready-for-agent\n");
    let world = bound_to(&named);
    world.issue(7, &["ready-for-agent", "bug"]);
    // An epic stays out, although it carries the workflow's label.
    world.issue(8, &["ready-for-agent", "epic"]);
    assert_eq!(frontier(&world), [(7, "build".to_string())]);
    stage(&world, 7, 1000, "build", "escalate");
    // The escalate label in place of the ready label, which is its flow label here.
    assert_eq!(world.labels(7), ["bug", "stage:build", "stuck"]);
    let ruled = world.afk(&["rule", "--issue", "7", "--text", "Go on."]).ok();
    assert_eq!((&ruled["relabelled"], &ruled["workflow"]), (&json!(true), &json!("feature")));
    assert!(has(&world, 7, "ready-for-agent") && !has(&world, 7, "stuck"));
    assert_eq!(frontier(&world), [(7, "build".to_string())]);
    assert!(!held(&world, 8));
}

#[test]
fn both_rework_counts_are_taken_from_after_the_ruling() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    stage(&world, 7, 1100, "review", "reject");
    stage(&world, 7, 1200, "build", "pass");
    // `max_rework` is 1: a second rejection is the one its breaker refuses.
    stage(&world, 7, 1300, "review", "reject");
    let tripped = show(&world, 7);
    assert_eq!(tripped["position"]["is"], "escalating");
    assert!(tripped["position"]["reason"].as_str().unwrap().contains("`max_rework`"));
    assert_eq!(tripped["rework"], json!({"had": 1, "left": 0}));
    assert!(has(&world, 7, "stuck") && !has(&world, 7, "flow:feature"));

    world
        .afk(&["rule", "--issue", "7", "--text", "The reviewer is right: handle the empty list."])
        .ok();
    let ruled = show(&world, 7);
    assert_eq!(
        (&ruled["position"]["is"], &ruled["position"]["stage"]),
        (&json!("perform"), &json!("build"))
    );
    assert_eq!(ruled["rework"], json!({"had": 0, "left": 1}));
    assert_eq!(ruled["consecutive_rework"]["review"], json!({"had": 0, "left": 2}));

    // The fix gets a fresh allowance: one more rejection is rework, not a breaker.
    stage(&world, 7, 1400, "build", "pass");
    stage(&world, 7, 1500, "review", "reject");
    assert_eq!(at(&world, 7), ("perform".to_string(), "build".to_string()));
    assert_eq!(show(&world, 7)["rework"], json!({"had": 1, "left": 0}));
    assert!(has(&world, 7, "flow:feature"));
}

/// Runs `args` as the host's watch does: the fleet started it on a harness,
/// and its environment names that harness and model beside its own marker.
fn as_the_watch(world: &World, args: &[&str]) -> Value {
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args)
        .env("AFKCONVOY_WATCH", "host")
        .env("AFKCONVOY_WATCH_SESSION", "1793693613-47457-18754")
        .env("AFKCONVOY_HARNESS", "claude-code")
        .env("AFKCONVOY_MODEL", "opus")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_watch_rules_and_sends_as_the_operator_whatever_harness_it_runs_on() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    stage(&world, 7, 1100, "review", "reject");
    stage(&world, 7, 1200, "build", "pass");
    stage(&world, 7, 1300, "review", "reject");
    assert_eq!(show(&world, 7)["position"]["is"], "escalating");

    let ruling = world.root.join("ruling.md");
    write(&world.root, "ruling.md", "The reviewer is right: handle the empty list.");
    let ruled =
        as_the_watch(&world, &["rule", "--issue", "7", "--text-file", ruling.to_str().unwrap()]);
    assert_eq!((&ruled["ruled"], &ruled["stage"]), (&json!(true), &json!("build")), "{ruled}");
    let record = records(&world, 7).pop().unwrap();
    assert!(
        record.starts_with("<!--convoy:stage stage=build verdict=ruling by=human-->\n"),
        "{record}"
    );
    let read = show(&world, 7);
    let last = read["records"].as_array().unwrap().last().unwrap();
    assert_eq!((&last["by"], &last["harness"]), (&json!("human"), &Value::Null), "{last}");
    assert_eq!(read["rework"], json!({"had": 0, "left": 1}));
    assert_eq!(read["consecutive_rework"]["review"], json!({"had": 0, "left": 2}));

    let sent =
        as_the_watch(&world, &["send", "--issue", "7", "--stage", "review", "--text", "Built."]);
    assert_eq!(sent["sent"], true, "{sent}");
    let record = records(&world, 7).pop().unwrap();
    assert!(
        record.starts_with("<!--convoy:stage stage=review verdict=sent by=human-->\n"),
        "{record}"
    );
}

#[test]
fn a_ruling_answers_a_reviewer_that_declared_escalation() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    // A declared escalation is actionable; silence alone waits for diagnosis.
    stage(&world, 7, 1400, "review", "escalate");
    assert!(has(&world, 7, "stuck") && !has(&world, 7, "flow:feature"));
    assert_eq!(at(&world, 7), ("escalating".to_string(), "review".to_string()));

    let ruled = world.afk(&["rule", "--issue", "7", "--text", "Review it again."]).ok();
    assert_eq!((&ruled["stage"], &ruled["relabelled"]), (&json!("review"), &json!(true)));
    assert!(has(&world, 7, "flow:feature") && !has(&world, 7, "stuck"));
    assert_eq!(did(&tick(&world, 2000)), ["dispatch 7 done"]);
    assert!(brief(&world, 7).contains("## Ruling\n\nThis stage runs again under a human's ruling"));
}

#[test]
fn a_ruling_is_refused_where_there_is_nothing_to_rule_on() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["bug"]);
    let refused =
        |issue: &str, text: &str| world.afk(&["rule", "--issue", issue, "--text", text]).error();
    let healthy = refused("7", "Go on.");
    assert!(healthy.contains("issue #7 is not escalated: it is at the stage `build`"), "{healthy}");
    assert!(refused("7", " \n").contains("carries the ruling: there is none"));
    let unfollowed = refused("8", "Go on.");
    assert!(unfollowed.contains("issue #8 cannot be ruled on: it carries no flow label"));
    assert_eq!(world.comments(7), Vec::<String>::new());
    assert_eq!(world.labels(8), ["bug"]);
    // An issue past its stages has none to run again.
    stage(&world, 7, 1000, "build", "already-satisfied");
    world.issue(9, &["flow:feature"]);
    world.comment(9, "<!--convoy:stage stage=build verdict=already-satisfied by=human-->\n\nDone.");
    assert!(refused("9", "Go on.").contains("it has left the stages of its workflow"));
}

#[test]
fn an_issue_is_sent_to_a_named_stage_of_its_workflow() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    let_go(&world, 7);
    assert_eq!(frontier(&world), [(7, "review".to_string())]);

    // A stage the workflow does not have is refused, and nothing is written.
    let unknown =
        world.afk(&["send", "--issue", "7", "--stage", "deploy", "--text", "Ship it."]).error();
    assert!(
        unknown.contains("has no stage `deploy`; it has: build, review, sign-off"),
        "{unknown}"
    );
    assert_eq!(records(&world, 7).len(), 1);

    let why = "The approach changed: start over.";
    let sent = world.afk(&["send", "--issue", "7", "--stage", "build", "--text", why]).ok();
    assert_eq!(
        (&sent["stage"], &sent["sent"], &sent["relabelled"]),
        (&json!("build"), &json!(true), &json!(false))
    );
    let record = records(&world, 7).pop().unwrap();
    assert!(
        record.starts_with(
            "<!--convoy:stage stage=build verdict=sent by=human-->\n**afkconvoy** — a human \
             sent this issue to the stage `build`.\n\n"
        ),
        "{record}"
    );
    let read = show(&world, 7);
    let last = read["records"].as_array().unwrap().last().unwrap();
    assert_eq!((&last["by"], &last["reads"]), (&json!("human"), &json!("sent")));
    assert_eq!(frontier(&world), [(7, "build".to_string())]);

    // The next tick starts the stage it was sent to, told why.
    assert_eq!(did(&tick(&world, 1100)), ["dispatch 7 done"]);
    let told = brief(&world, 7);
    assert!(told.contains("## Sent here\n") && told.contains(&format!("> {why}")), "{told}");
    // While a worker is at it, it is not sent anywhere.
    let held = world.afk(&["send", "--issue", "7", "--stage", "review", "--text", "x"]).error();
    assert!(held.contains("issue #7 is claimed"), "{held}");
}

#[test]
fn sending_an_escalated_issue_hands_it_back_to_the_fleet() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    stage(&world, 7, 1100, "review", "escalate");
    assert!(has(&world, 7, "stuck") && !has(&world, 7, "flow:feature"));

    let sent =
        world.afk(&["send", "--issue", "7", "--stage", "build", "--text", "Rebuild it."]).ok();
    assert_eq!(sent["relabelled"], true);
    assert!(has(&world, 7, "flow:feature") && !has(&world, 7, "stuck"));
    assert_eq!(frontier(&world), [(7, "build".to_string())]);
    // To a human's stage it waits for them, and no worker is started.
    stage(&world, 7, 1200, "build", "pass");
    let_go(&world, 7);
    world.afk(&["send", "--issue", "7", "--stage", "sign-off", "--text", "I read it."]).ok();
    assert_eq!(at(&world, 7), ("waiting".to_string(), "sign-off".to_string()));
    assert_eq!(did(&tick(&world, 1300)), [] as [&str; 0]);
}

#[test]
fn an_issue_at_a_human_s_stage_is_not_dispatched_and_holds_no_claim_and_no_slot() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    stage(&world, 7, 1100, "review", "pass");
    assert_eq!(at(&world, 7), ("waiting".to_string(), "sign-off".to_string()));
    assert_eq!(show(&world, 7)["position"]["by"], "human");
    assert_eq!(world.claims(), []);
    // The status board says who it waits for.
    let board = world.boards(7).pop().unwrap();
    assert!(board.contains("the issue waits for a human"), "{board}");
    assert!(board.contains("the stage `sign-off` that follows is a human's"), "{board}");

    // Nothing is started on it, tick after tick, and it takes none of the
    // one slot: another issue is worked meanwhile.
    let idle = tick(&world, 1200);
    assert_eq!((did(&idle), &idle["held"]), (vec![], &json!(0)));
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let row = plan["excluded"].as_array().unwrap().iter().find(|row| row["number"] == 7).unwrap();
    assert!(row["reason"].as_str().unwrap().contains("waits for a human"), "{row}");
    assert_eq!(plan["free_slots"], 1);
    world.issue(8, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1300)), ["dispatch 8 done"]);
    assert_eq!(world.claims(), [(8, "alpha".to_string())]);
    assert_eq!(world.labels(7), ["flow:feature", "stage:sign-off"]);
}

#[test]
fn a_human_declares_the_verdict_of_their_stage_with_the_verdict_command() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    stage(&world, 7, 1000, "build", "pass");
    stage(&world, 7, 1100, "review", "pass");
    let declare = |verdict: &str, stage: &str| {
        world.afk(&["verdict", verdict, "--issue", "7", "--stage", stage, "--text", "Read it."])
    };

    // It is checked before it is written: a human reads the refusal.
    let elsewhere = declare("pass", "review").error();
    assert!(elsewhere.contains("issue #7 is at the stage `sign-off`, not at `review`"));
    let parked = world.afk(&[
        "verdict",
        "blocked",
        "--issue",
        "7",
        "--stage",
        "sign-off",
        "--text",
        "x",
        "--blocked-by",
        "3",
    ]);
    assert!(parked.error().contains("--blocked-by is a worker's"));
    assert_eq!(records(&world, 7).len(), 2);

    // A rejection leads where the workflow says, and is rework like any other.
    let rejected = declare("reject", "sign-off").ok();
    assert_eq!(
        (&rejected["declared"], &rejected["by"], &rejected["verdict"]),
        (&json!(true), &json!("human"), &json!("reject"))
    );
    let record = records(&world, 7).pop().unwrap();
    assert!(
        record.starts_with(
            "<!--convoy:stage stage=sign-off verdict=reject by=human-->\n**afkconvoy** — a \
             human declared `reject` for the stage `sign-off`.\n\nRead it."
        ),
        "{record}"
    );
    assert_eq!(frontier(&world), [(7, "build".to_string())]);
    assert_eq!(show(&world, 7)["rework"], json!({"had": 1, "left": 0}));
    stage(&world, 7, 1200, "build", "pass");
    // While a worker holds the issue its stage is the worker's to declare.
    let held = declare("pass", "review").error();
    assert!(held.contains("issue #7 is claimed") && held.contains("--role-commit"), "{held}");
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let line = ["verdict", "pass", "--issue", "7", "--stage", "review", "--text", "Fine."];
    world.afk(&[&line[..], &commits].concat()).ok();
    assert_eq!(did(&tick(&world, 1330)), ["advance 7 done"]);

    // A pass ends the workflow here; what it declared is on the record as a human's.
    assert_eq!(declare("pass", "sign-off").ok()["declared"], true);
    let read = show(&world, 7);
    assert_eq!(read["position"]["is"], "ended");
    let by: Vec<&str> =
        read["records"].as_array().unwrap().iter().map(|r| r["by"].as_str().unwrap()).collect();
    assert_eq!(by, ["worker", "worker", "human", "worker", "worker", "human"]);
    assert_eq!(did(&tick(&world, 1400)), [] as [&str; 0]);
    let gone = declare("pass", "sign-off").error();
    assert!(gone.contains("issue #7 is at no stage: it has left them"), "{gone}");
}

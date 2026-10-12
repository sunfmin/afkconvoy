//! Stage records (docs/adr/0010): a stage ends with `verdict`, which leaves
//! one record on the issue; where the issue is can be read off its records
//! with `show --issue`; and each tick brings its `stage:` label to that
//! position, reading the label for nothing.

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Answer, World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: done, reject: build }
```
";
const CONFIG: &str = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";

/// A world whose repo is bound to a team with the one workflow `feature`.
fn bound() -> World {
    let world = World::new();
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", "build\n")]);
    world.repo("acme/reviewer", &[(".claude/skills/review/SKILL.md", "review\n")]);
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

/// What a tick did about the `stage:` labels.
fn labelled(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    steps
        .filter(|step| step["do"] == "stage_label")
        .map(|step| format!("{} {}", step["issue"], step["outcome"].as_str().unwrap()))
        .collect()
}

/// Claims `issue` for alpha at `now`, as a dispatch to a stage does → the claim.
fn claim(world: &World, issue: u64, now: u64) -> String {
    let (issue, now) = (issue.to_string(), now.to_string());
    let args = ["claim", "--issue", &issue, "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()["sha"].as_str().unwrap().to_string()
}

/// Ends alpha's claim on `issue`, as the fleet does once a stage has declared.
fn release(world: &World, issue: u64) {
    world.afk(&["release", "--issue", &issue.to_string(), "--instance", "alpha"]).ok();
}

/// Declares `verdict` for `stage` of issue 7, under the commits `r1` and `t1`.
fn declare(world: &World, stage: &str, verdict: &str, more: &[&str]) -> Answer {
    let line = ["verdict", verdict, "--issue", "7", "--stage", stage];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    world.afk(&[&line[..], &commits, more].concat())
}

/// One whole stage of issue 7: claimed at `now`, declared, the claim ended.
fn stage(world: &World, now: u64, stage: &str, verdict: &str) -> String {
    let claim = claim(world, 7, now);
    assert_eq!(declare(world, stage, verdict, &["--text", "done"]).ok()["declared"], true);
    release(world, 7);
    claim
}

fn show(world: &World) -> Value {
    world.afk(&["show", "--issue", "7"]).ok()
}

fn stage_labels(world: &World, issue: u64) -> Vec<String> {
    world.labels(issue).into_iter().filter(|label| label.starts_with("stage:")).collect()
}

#[test]
fn a_verdict_leaves_one_stage_record_carrying_the_stage_the_claim_and_the_commits() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    let claim = claim(&world, 7, 1000);
    world.gh_calls();

    let text = "## Built\n\nTwo files, and the gate is green.";
    let declared = declare(&world, "build", "pass", &["--text", text]).ok();
    assert_eq!(
        declared,
        json!({"issue": 7, "stage": "build", "verdict": "pass", "claim": claim,
            "declared": true, "woke": false})
    );
    assert_eq!(
        world.comments(7),
        [format!(
            "<!--convoy:stage stage=build verdict=pass claim={claim} role_commit=r1 \
             team_commit=t1-->\n**afkconvoy** — the stage `build` declared `pass`.\n\n{text}"
        )]
    );
    // It moved nothing: no label, and the claim is the fleet's to end.
    let wrote = world.gh_calls().into_iter().filter(|call| call.contains(&"--method".to_string()));
    assert_eq!(wrote.count(), 1, "one comment, and nothing else written");
    assert_eq!(world.labels(7), ["flow:feature"]);
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
}

#[test]
fn declaring_twice_for_one_stage_under_one_claim_leaves_one_record() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    claim(&world, 7, 1000);
    assert_eq!(declare(&world, "build", "pass", &["--text", "built"]).ok()["declared"], true);

    for verdict in ["pass", "escalate"] {
        let again = declare(&world, "build", verdict, &["--text", "again"]).ok();
        assert_eq!((&again["declared"], &again["standing"]), (&json!(false), &json!("pass")));
    }
    assert_eq!(world.comments(7).len(), 1);
    assert_eq!(show(&world)["position"]["stage"], "review");

    // The same stage under another claim — it ran again — is another record.
    release(&world, 7);
    claim(&world, 7, 2000);
    assert_eq!(declare(&world, "build", "pass", &["--text", "rebuilt"]).ok()["declared"], true);
    assert_eq!(world.comments(7).len(), 2);
}

#[test]
fn the_brief_s_line_declares_from_the_role_s_directory_with_the_commits_of_the_environment() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    let orca = world.orca();
    let worktree = PathBuf::from(orca["worktrees"][0]["path"].as_str().unwrap());
    let git_dir = PathBuf::from(git(&worktree, &["rev-parse", "--absolute-git-dir"]));
    let brief = std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap();
    let line = brief.lines().find(|line| line.contains(" verdict --issue 7")).unwrap();
    assert!(line.starts_with(&format!("cd '{}' && ", worktree.display())), "{line}");

    // A worker is started in its role's copy, which is no checkout of the repo.
    let elsewhere = world.root.join("role-copy");
    std::fs::create_dir(&elsewhere).unwrap();
    let ran = world
        .command("sh", &elsewhere)
        .args(["-c", &format!("{line} pass --text built")])
        .env("AFKCONVOY_ROLE_COMMIT", "role-abc")
        .env("AFKCONVOY_TEAM_COMMIT", "team-def")
        .output()
        .unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
    let record = &world.comments(7)[1];
    assert!(record.contains(" stage=build verdict=pass "), "{record}");
    assert!(record.contains(" role_commit=role-abc team_commit=team-def-->"), "{record}");
    assert_eq!(show(&world)["records"][0]["reads"], "verdict");
}

#[test]
fn a_verdict_wakes_the_fleet() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    let mut running = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["run", "--instance", "alpha", "--host", "h1", "--ticks", "2"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(running.stdout.take().unwrap()).lines();
    let first = lines.next().unwrap().unwrap();
    assert!(
        first.starts_with("tick 1: dispatched #7") && first.ends_with("next in 90s"),
        "{first}"
    );

    let started = Instant::now();
    assert_eq!(declare(&world, "build", "pass", &["--text", "built"]).ok()["woke"], true);
    let second = lines.next().unwrap().unwrap();
    assert!(second.starts_with("tick 2: "), "{second}");
    assert!(started.elapsed() < Duration::from_secs(20), "the sleep was not waited out");
    running.wait().unwrap();
    // …and the tick it opened read the record: the label followed.
    assert_eq!(stage_labels(&world, 7), ["stage:review"]);
}

#[test]
fn what_is_no_verdict_or_carries_nothing_is_refused_and_writes_nothing() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    let refused = |args: &[&str]| declare(&world, "build", args[0], &args[1..]).error();

    let unclaimed = refused(&["pass", "--text", "built"]);
    assert!(unclaimed.contains("issue #7 is not claimed"), "{unclaimed}");
    claim(&world, 7, 1000);
    for no_verdict in ["ruling", "needs-human", "done"] {
        let why = refused(&[no_verdict, "--text", "x"]);
        assert!(why.contains("a verdict is one of: pass, reject, blocked,"), "{why}");
    }
    assert!(refused(&["pass"]).contains("--text"));
    assert!(refused(&["pass", "--text", " \n"]).contains("there is none"));
    let why = refused(&["pass", "--text", "x", "--blocked-by", "3"]);
    assert!(why.contains("--blocked-by goes with `blocked` only"), "{why}");
    // The commits are the fleet's to say: with neither a flag nor the environment, nothing.
    let bare = world.afk(&["verdict", "pass", "--issue", "7", "--stage", "build", "--text", "x"]);
    assert!(bare.error().contains("--role-commit"));
    assert_eq!(world.comments(7), Vec::<String>::new());

    let file = world.root.join("why.md");
    std::fs::write(&file, "Waiting on the schema.\n").unwrap();
    let blocked = ["--text-file", file.to_str().unwrap(), "--blocked-by", "3,4"];
    assert_eq!(declare(&world, "build", "blocked", &blocked).ok()["declared"], true);
    let record = &world.comments(7)[0];
    assert!(record.contains(" blocked_by=3,4-->"), "{record}");
    assert!(record.ends_with("\n\nWaiting on the schema."), "{record}");
}

#[test]
fn show_says_where_an_issue_is_what_follows_and_the_rework_it_has_left() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    let entered = show(&world);
    assert_eq!(entered["workflow"], "feature");
    assert_eq!(
        entered["position"],
        json!({"is": "perform", "stage": "build", "by": "dev", "do": "implement", "reason": null})
    );
    assert_eq!(entered["follows"]["stage"], "review");
    assert_eq!(entered["rework"], json!({"had": 0, "left": 3}));
    assert_eq!(entered["consecutive_rework"], json!({"review": {"had": 0, "left": 2}}));
    assert_eq!(entered["records"], json!([]));

    let built = stage(&world, 1000, "build", "pass");
    stage(&world, 2000, "review", "reject");
    let back = show(&world);
    assert_eq!(
        (&back["position"]["is"], &back["position"]["stage"]),
        (&json!("perform"), &json!("build"))
    );
    assert_eq!(back["rework"], json!({"had": 1, "left": 2}));
    assert_eq!(back["consecutive_rework"], json!({"review": {"had": 1, "left": 1}}));
    let first = &back["records"][0];
    assert_eq!(
        (&first["stage"], &first["verdict"], &first["reads"], &first["claim"]),
        (&json!("build"), &json!("pass"), &json!("verdict"), &json!(built))
    );
    assert_eq!((&first["role_commit"], &first["team_commit"]), (&json!("r1"), &json!("t1")));
    assert!(first["url"].as_str().unwrap().starts_with("https://gh/c/"), "{first}");

    stage(&world, 3000, "build", "pass");
    stage(&world, 4000, "review", "pass");
    let ended = show(&world);
    assert_eq!((&ended["position"]["is"], &ended["follows"]), (&json!("ended"), &Value::Null));

    // An issue that follows no workflow of the team has no position.
    world.issue(8, &["ready-for-agent"]);
    world.issue(9, &["flow:chore"]);
    let none = world.afk(&["show", "--issue", "8"]).error();
    assert!(none.contains("issue #8 has no position: it carries no flow label"), "{none}");
    let unknown = world.afk(&["show", "--issue", "9"]).error();
    assert!(unknown.contains("flow:chore names no workflow of the team"), "{unknown}");
    assert!(
        world.afk(&["show", "feature", "--issue", "7"]).error().contains("cannot be used with")
    );
}

#[test]
fn a_record_whose_stage_was_not_dispatched_under_its_claim_is_not_a_verdict() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    claim(&world, 7, 1000);
    // The stage that was dispatched declares, and that ends the claim's say:
    // the same worker does not declare the stage that follows.
    declare(&world, "build", "pass", &["--text", "built"]).ok();
    declare(&world, "review", "pass", &["--text", "and reviewed, by the same worker"]).ok();
    assert_eq!(show(&world)["position"]["stage"], "review");
    // The issue is at `review`: under the next claim, a record of any other
    // stage moves nothing.
    release(&world, 7);
    claim(&world, 7, 2000);
    declare(&world, "build", "pass", &["--text", "built again, unasked"]).ok();
    let read = show(&world);
    assert_eq!(read["position"]["stage"], "review", "{read}");
    let reads: Vec<&str> =
        read["records"].as_array().unwrap().iter().map(|r| r["reads"].as_str().unwrap()).collect();
    assert_eq!(reads, ["verdict", "claim-spent", "other-stage"]);
    // A marker typed into prose, or below anything else, is no record at all.
    world.comment(
        7,
        "> <!--convoy:stage stage=review verdict=pass claim=x role_commit=r team_commit=t-->",
    );
    assert_eq!(show(&world)["records"].as_array().unwrap().len(), 3);
}

#[test]
fn each_tick_brings_the_stage_label_to_the_position_and_reads_it_for_nothing() {
    let world = bound();
    world.issue(7, &["flow:feature", "bug"]);
    world.issue(8, &["ready-for-agent"]);
    world.afk(&["plan"]).ok();
    assert_eq!(stage_labels(&world, 7), Vec::<String>::new(), "a plan writes nothing");

    let first = tick(&world, 1000);
    assert_eq!(labelled(&first), ["7 done"], "{first}");
    assert_eq!(world.labels(7), ["flow:feature", "bug", "stage:build"]);
    assert_eq!(world.labels(8), ["ready-for-agent"], "no flow label: left alone");
    // A second tick over an unchanged world writes nothing.
    world.gh_calls();
    assert_eq!(labelled(&tick(&world, 1090)), ["7 nothing"]);
    let wrote =
        world.gh_calls().into_iter().any(|call| call.iter().any(|arg| arg.ends_with("/labels")));
    assert!(!wrote);

    // Moved by hand, it changes nothing and the next tick moves it back.
    world.with_gh(|gh| {
        gh["issues"][0]["labels"] = json!(["flow:feature", "stage:review", "stage:x y"])
    });
    assert_eq!(show(&world)["position"]["stage"], "build");
    assert_eq!(labelled(&tick(&world, 1180)), ["7 done"]);
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);

    // A verdict moves the position, and the label follows it.
    declare(&world, "build", "pass", &["--text", "built"]).ok();
    assert_eq!(labelled(&tick(&world, 1270)), ["7 done"]);
    assert_eq!(stage_labels(&world, 7), ["stage:review"]);

    // Ended, it has no stage, so it carries no `stage:` label.
    release(&world, 7);
    stage(&world, 5000, "review", "pass");
    tick(&world, 5090);
    assert_eq!(stage_labels(&world, 7), Vec::<String>::new());
}

#[test]
fn a_label_that_cannot_be_derived_fails_its_step_and_nothing_else() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(9, &["flow:chore"]);
    let ticked = tick(&world, 1000);
    // A flow label that names no workflow gives no position to show: the
    // issue is handed to a human, and follows no workflow from then on.
    assert_eq!(labelled(&ticked), ["7 done", "9 nothing"], "{ticked}");
    assert_eq!(world.labels(9), ["ready-for-human"]);

    // A team that no longer loads: the labels are left as they are, and said so.
    let team = world.root.join("github/acme/team");
    common::commit(&team, "workflows/feature.md", "```yaml\nstages: []\n```\n");
    let broken = tick(&world, 1090);
    assert_eq!(labelled(&broken), ["7 failed"], "{broken}");
    let errors = broken["errors"].as_array().unwrap();
    assert!(errors[0]["error"].as_str().unwrap().contains("the team acme/team has"), "{broken}");
    assert_eq!(stage_labels(&world, 7), ["stage:build"]);
    assert!(Path::new(&world.clone).exists());
}

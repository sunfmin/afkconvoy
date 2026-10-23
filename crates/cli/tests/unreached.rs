//! A read of GitHub that failed in transit: it is made again before anything
//! is said, named for what it is when it still fails, said once a tick however
//! many steps it stopped, and no fault of the team. A step is stopped by it
//! only where it needs the role that could not be read.

use crate::common;

use common::{World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  research: acme/research\n```\n";
const FIX: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: check }
  - name: check
    by: dev
    do: implement
    on: { pass: done }
```
";
const ROUNDS: &str = "```yaml\nby: research\ndo: rounds\nevery: 1d\nmax_open: 2\n```\n";

/// The read of the head of `repo`, as the stand-in `gh` is asked for it.
fn head_of(repo: &str) -> String {
    format!("repos/{repo}/commits/HEAD")
}

/// A world whose repo is bound to a team of two roles: `dev` performs both
/// stages of the workflow `fix`, and `research` the patrol `rounds` and no
/// stage. The repo names `patrols` of them.
fn bound(patrols: &str) -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    world.repo(
        "acme/research",
        &[(".claude/skills/rounds/SKILL.md", "---\nname: rounds\ndescription: rounds\n---\n")],
    );
    let team = [("team.md", TEAM), ("workflows/fix.md", FIX), ("patrols/rounds.md", ROUNDS)];
    world.repo("acme/team", &team);
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
         patrols: {patrols}\n```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// Cuts short the next `times` reads whose command line holds `call`.
fn cut_short(world: &World, call: &str, times: u64) {
    world.with_gh(|gh| gh["cut_short"] = json!({call: times}));
}

/// How many more reads the stand-in `gh` is still to cut short.
fn left_to_cut(world: &World) -> u64 {
    let cuts = world.gh()["cut_short"].clone();
    cuts.as_object().unwrap().values().map(|left| left.as_u64().unwrap()).sum()
}

fn run(world: &World, ticks: &str) -> Vec<String> {
    let run = ["run", "--instance", "alpha", "--host", "h1"];
    world
        .afk_lines(&[&run[..], &["--second-ms", "0", "--now", "1000", "--ticks", ticks]].concat())
        .0
}

fn tick(world: &World, now: &str) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]).ok()
}

fn stage_labels(world: &World, issue: u64) -> Vec<String> {
    world.labels(issue).into_iter().filter(|label| label.starts_with("stage:")).collect()
}

#[test]
fn a_role_read_that_fails_once_and_then_succeeds_fails_no_step_and_prints_nothing() {
    let world = bound("[rounds]");
    world.issue(7, &["flow:fix"]);
    cut_short(&world, &head_of("acme/research"), 1);
    let lines = run(&world, "1");
    assert_eq!(left_to_cut(&world), 0, "the read was cut short");
    assert!(!lines[0].contains("FAILED"), "{lines:?}");
    assert!(!lines[0].contains("could not be reached"), "{lines:?}");
    assert!(lines[0].contains("dispatched #7"), "{lines:?}");
    assert!(lines[0].contains("ran the patrol rounds"), "{lines:?}");
}

#[test]
fn a_read_that_fails_three_times_is_one_line_for_the_tick_and_the_next_tick_starts_with_no_sleep() {
    let world = bound("[]");
    world.issue(7, &["flow:fix", "stage:check"]);
    world.issue(8, &["flow:fix", "stage:check"]);
    // the team's own repo: no issue of the tick has a position without it.
    // The start reads it first, for the harnesses it calls for, and goes on
    // without: three tries are the start's, and three the first tick's.
    cut_short(&world, &head_of("acme/team"), 6);
    let lines = run(&world, "2");
    assert_eq!(left_to_cut(&world), 0, "three tries a read, and no more");
    let cause = "GitHub could not be reached: gh api repos/acme/team/commits/HEAD: unexpected \
                 end of JSON input";
    assert!(lines[0].contains(&format!("FAILED 2 steps: {cause}")), "{lines:?}");
    assert_eq!(lines[0].matches("could not be reached").count(), 1, "said once: {lines:?}");
    assert!(!lines[0].contains("fault"), "no fault of the team: {lines:?}");
    // nothing says the next read fails too: it is made at once, and does not
    assert!(lines[0].ends_with(" · next now"), "{lines:?}");
    assert!(lines[1].contains("dispatched #7, #8"), "{lines:?}");
    assert!(!lines[1].contains("FAILED"), "{lines:?}");

    // `doctor` keeps the cause once, with the steps it stopped
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    let failed = doctor["fleet"]["failed_steps"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!((&failed[0]["tick"], &failed[0]["count"]), (&json!(1), &json!(2)));
    assert_eq!(failed[0]["error"], cause);
}

#[test]
fn a_github_that_stays_out_of_reach_does_not_make_the_loop_spin() {
    let world = bound("[]");
    world.issue(7, &["flow:fix", "stage:check"]);
    cut_short(&world, &head_of("acme/team"), 99);
    let lines = run(&world, "5");
    let nexts: Vec<&str> =
        lines.iter().map(|line| line.rsplit(" · ").next().unwrap_or_default()).collect();
    assert_eq!(nexts, ["next now", "next now", "next now", "next in 90s", "next now"]);
}

#[test]
fn a_role_that_does_not_exist_is_still_a_fault_of_the_team() {
    let world = bound("[]");
    world.issue(7, &["flow:fix", "stage:check"]);
    world.with_gh(|gh| {
        gh["repos"].as_object_mut().unwrap().remove("acme/research");
    });
    // a fault is said on a line of its own
    let line = run(&world, "1").join("\n");
    assert!(line.contains("the team acme/team has 1 fault"), "{line}");
    assert!(line.contains("team.md: roles.research: acme/research cannot be read"), "{line}");
    assert!(!line.contains("could not be reached"), "{line}");
    assert!(line.ends_with(" · next in 90s"), "{line}");
}

#[test]
fn with_a_role_out_of_reach_an_issue_whose_workflow_does_not_name_it_is_advanced_and_labelled() {
    let world = bound("[rounds]");
    world.issue(7, &["flow:fix"]);
    cut_short(&world, &head_of("acme/research"), 99);
    let ticked = tick(&world, "1000");
    // only the patrol `research` performs is stopped, and it says why
    let errors = ticked["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{ticked}");
    assert_eq!((&errors[0]["do"], &errors[0]["patrol"]), (&json!("patrol"), &json!("rounds")));
    let error = errors[0]["error"].as_str().unwrap();
    assert!(
        error.starts_with("GitHub could not be reached: gh api repos/acme/research"),
        "{error}"
    );
    assert_eq!(stage_labels(&world, 7), ["stage:build"]);

    let declare = ["verdict", "pass", "--issue", "7", "--stage", "build", "--text", "built"];
    world.afk(&[&declare[..], &["--role-commit", "r1", "--team-commit", "t1"]].concat()).ok();
    let ticked = tick(&world, "1100");
    let advanced = ticked["did"].as_array().unwrap().iter().find(|step| step["do"] == "advance");
    let advanced = advanced.unwrap_or_else(|| panic!("an advance: {ticked}"));
    assert_eq!((&advanced["issue"], &advanced["stage"]), (&json!(7), &json!("check")), "{ticked}");
    assert_eq!(stage_labels(&world, 7), ["stage:check"]);
}

#[test]
fn a_command_that_answers_for_the_whole_team_says_a_role_is_out_of_reach_and_names_no_fault() {
    let world = bound("[]");
    cut_short(&world, &head_of("acme/research"), 99);
    let error = world.afk(&["check"]).error();
    assert!(
        error.starts_with("GitHub could not be reached: gh api repos/acme/research"),
        "{error}"
    );
    assert!(!error.contains("fault"), "{error}");
}

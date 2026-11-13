//! The tick walks an issue through the stages of its workflow (docs/adr/0010):
//! a verdict ends its stage's claim, and the tick that reads it starts the
//! stage the records then put the issue at — in the same worktree, under a
//! claim of its own, with what was declared so far as the worker's history.
//! With no slot free, that start is a later tick's.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
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

/// What a tick did about issue 7 and the ones after it that came to
/// anything, but for the boards and the `stage:` labels.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let kept = steps.filter(|step| {
        !["board", "stage_label"].contains(&step["do"].as_str().unwrap())
            && step["issue"].as_u64().is_some_and(|issue| issue >= 7)
            && step["outcome"] != "nothing"
    });
    kept.map(|step| {
        format!("{} {} {}", step["do"].as_str().unwrap(), step["issue"], step["outcome"])
    })
    .map(|line| line.replace('"', ""))
    .collect()
}

/// Declares `verdict` for `stage` of `issue`, as its worker would.
fn declare(world: &World, issue: u64, stage: &str, verdict: &str, more: &[&str]) {
    let issue = issue.to_string();
    let line = ["verdict", verdict, "--issue", &issue, "--stage", stage];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let declared = world.afk(&[&line[..], &commits, more].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The commands of the terminals open in `dir`.
fn workers(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).map(|t| t["command"].as_str().unwrap().to_string()).collect()
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

/// The frontier `plan` reports: each issue with the stage it is at.
fn frontier(world: &World) -> Vec<(u64, String)> {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let rows = plan["frontier"].as_array().unwrap().iter();
    rows.map(|row| (row["number"].as_u64().unwrap(), row["stage"].as_str().unwrap().to_string()))
        .collect()
}

/// Why `plan` leaves `issue` off the frontier.
fn excluded(world: &World, issue: u64) -> String {
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    let rows = plan["excluded"].as_array().unwrap();
    let row = rows.iter().find(|row| row["number"] == issue).unwrap_or(&Value::Null);
    row["reason"].as_str().unwrap_or_default().to_string()
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

#[test]
fn a_pass_starts_the_next_stage_s_worker_in_the_same_worktree_and_done_ends_the_workflow() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["ready-for-agent"]);
    assert_eq!(frontier(&world), [(7, "build".to_string())], "no flow label: never touched");
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let dir = worktree(&world, 7);
    assert!(workers(&world, &dir)[0].contains(" AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' "));
    assert!(brief(&dir).contains("None yet: no stage of this issue has declared a verdict."));

    // Opening a PR declares nothing: the issue is as it was, and so is its worker.
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    assert_eq!(did(&tick(&world, 1090)), [] as [&str; 0]);
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);

    // A `pass` ends the stage's claim, and the same tick starts a worker in
    // the role of the stage that follows, where the last one worked, under a
    // claim of its own — the issue's PR open or not.
    declare(&world, 7, "build", "pass", &["--text", "Built: seven.txt holds it."]);
    let advanced = tick(&world, 1180);
    assert_eq!(did(&advanced), ["advance 7 done", "dispatch 7 done"]);
    let advance = advanced["did"].as_array().unwrap().iter().find(|step| step["do"] == "advance");
    assert_eq!(advance.unwrap()["stage"], "review", "{advanced}");
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert_eq!(frontier(&world), []);
    // The worker that declared is ended with its claim, and the worktree
    // kept for the stage that follows.
    assert_eq!(worktree(&world, 7), dir);
    let started = workers(&world, &dir);
    assert_eq!(started.len(), 1, "the new stage's, and no other: {started:?}");
    assert_eq!(world.open_terminals().len(), 1);
    assert!(started[0].contains(" AFKCONVOY_STAGE='review' AFKCONVOY_ROLE='reviewer' "));
    assert!(dir.join("seven.txt").is_file());
    let told = brief(&dir);
    assert!(told.contains("- **Stage:** `review`, of the workflow `feature`\n"), "{told}");
    assert!(told.contains("- `build` declared `pass` — https://gh/c/"), "{told}");
    assert!(!told.contains("## Rework"), "{told}");

    // The next tick has nothing to do for the issue, and one over an
    // unchanged world writes nothing.
    assert_eq!(did(&tick(&world, 1270)), [] as [&str; 0]);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);
    let wrote: Vec<_> = world
        .gh_calls()
        .into_iter()
        .filter(|call| call.contains(&"--method".to_string()))
        .collect();
    assert_eq!(wrote, [] as [Vec<String>; 0], "nothing written to GitHub");

    // A `pass` that leads to `done` ends the workflow: the claim is released,
    // and nobody is started on the issue again.
    declare(&world, 7, "review", "pass", &["--text", "It reads well."]);
    assert_eq!(did(&tick(&world, 1450)), ["advance 7 done"]);
    assert_eq!(world.claims(), []);
    // Nothing will use its worktree again: it goes, its worker with it. The
    // branch holds a commit past the base branch, and outlives it.
    assert_eq!(world.worktrees(), Vec::<PathBuf>::new());
    assert_eq!(world.open_terminals(), Vec::<String>::new());
    assert!(!dir.exists());
    assert!(git(&world.clone, &["branch", "--list", "tester/issue-7*"]).contains("issue-7"));
    assert_eq!(did(&tick(&world, 1540)), [] as [&str; 0]);
    assert_eq!(excluded(&world, 7), "its workflow has ended");
    assert_eq!(world.state(7), "open");
    assert_eq!(world.labels(8), ["ready-for-agent"]);
    assert_eq!(world.comments(8), Vec::<String>::new());
}

/// A workflow that opens no PR: its one stage's `pass` ends it.
const TRIAGE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: human }
    on: { pass: done }
```
";

#[test]
fn a_workflow_that_ends_with_nothing_on_its_branch_leaves_no_worktree_terminal_or_branch() {
    let world = bound_to(TRIAGE);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let dir = worktree(&world, 7);
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    // What its worker left uncommitted is nothing a later stage works from.
    std::fs::write(dir.join("notes.txt"), "scratch").unwrap();
    declare(&world, 7, "build", "pass", &["--text", "Answered on the issue."]);

    // The worker sits at its prompt still: the tick that reads its verdict ends it.
    assert_eq!(workers(&world, &dir).len(), 1);
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    assert_eq!(world.worktrees(), Vec::<PathBuf>::new());
    assert_eq!(world.open_terminals(), Vec::<String>::new());
    assert!(!dir.exists());
    assert_eq!(git(&world.clone, &["branch", "--list", &branch]), "");
    assert_eq!(world.state(7), "open", "it stays open until somebody closes it");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_reject_starts_the_stage_the_workflow_names_with_the_reason_in_its_brief() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);
    declare(&world, 7, "review", "reject", &["--text", "The empty list is not handled.\nAdd it."]);
    // The tick that reads the `reject` starts the stage it sends the issue
    // back to, and leaves the next tick nothing.
    assert_eq!(did(&tick(&world, 1180)), ["advance 7 done", "dispatch 7 done"]);
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 1270)), [] as [&str; 0]);
    let dir = worktree(&world, 7);
    assert!(workers(&world, &dir)[0].contains(" AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' "));
    let told = brief(&dir);
    assert!(told.contains("- `review` declared `reject` — https://gh/c/"), "{told}");
    assert!(told.contains("## Rework\n"), "{told}");
    assert!(told.contains("> The empty list is not handled.\n> Add it."), "{told}");
}

#[test]
fn a_commit_left_by_a_stage_marked_as_writing_nothing_escalates_the_issue() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done", "dispatch 8 done"]);
    for issue in [7, 8] {
        let dir = worktree(&world, issue);
        // A stage that may write leaves commits, and passes.
        commit(&dir, "work.txt", "built\n");
        declare(&world, issue, "build", "pass", &["--text", "Built."]);
    }
    let reviewing = ["advance 7 done", "dispatch 7 done", "advance 8 done", "dispatch 8 done"];
    assert_eq!(did(&tick(&world, 1090)), reviewing);

    // The reviewer of 7 fixes what it found — past its guard, in a script —
    // and the reviewer of 8 only reads. Both pass.
    let fixed = worktree(&world, 7);
    let before = git(&fixed, &["rev-parse", "HEAD"]);
    let after = commit(&fixed, "work.txt", "fixed while reviewing\n");
    // A worker that is started again in the stage answers for the one before it.
    let again = ["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1"];
    world.afk(&again).ok();
    declare(&world, 7, "review", "pass", &["--text", "Fixed a typo; it reads well."]);
    declare(&world, 8, "review", "pass", &["--text", "It reads well."]);
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done", "advance 8 done"]);

    // 7 is a human's, with the reason; 8 went where its workflow says.
    assert_eq!(world.labels(7), ["ready-for-human"]);
    let said = world.comments(7).join("\n");
    let why = format!(
        "The stage `review` is marked `writes: false`, and it left a commit on the branch: it \
         started at {before} and the branch is now at {after}."
    );
    assert!(said.contains(&why), "{said}");
    assert!(said.contains("Its stage record: https://gh/c/"), "{said}");
    assert_eq!(world.labels(8), ["flow:feature"]);
    assert_eq!(excluded(&world, 8), "its workflow has ended");
    assert_eq!(world.claims(), []);
    assert!(fixed.join("work.txt").is_file(), "the worktree is kept for the human");
    assert_eq!(workers(&world, &fixed), [] as [&str; 0], "and its worker is ended");
    assert_eq!(world.open_terminals(), Vec::<String>::new(), "nothing claimed, nothing alive");
    assert!(!world.comments(8).join("\n").contains("left a commit"));
}

#[test]
fn a_flow_label_that_names_no_workflow_escalates_the_issue_once() {
    let world = bound();
    world.issue(9, &["flow:chore", "bug"]);
    world.issue(10, &["flow:feature", "flow:chore"]);
    assert_eq!(frontier(&world), []);
    let ticked = tick(&world, 1000);
    assert_eq!(did(&ticked), ["escalate 9 done", "escalate 10 done"], "{ticked}");
    let said = world.comments(9).join("\n");
    assert!(said.contains("flow:chore names no workflow of the team"), "{said}");
    assert!(world.comments(10).join("\n").contains("2 flow labels (flow:feature, flow:chore)"));
    // It is a human's now: no flow label, the team's escalate label, and no claim.
    assert_eq!(world.labels(9), ["bug", "ready-for-human"]);
    assert_eq!(world.labels(10), ["ready-for-human"]);
    assert_eq!(world.claims(), []);
    assert_eq!(ticked["held"], 0);

    world.gh_calls();
    assert_eq!(did(&tick(&world, 1090)), [] as [&str; 0]);
    let comments = world.comments(9);
    assert!(comments.iter().any(|body| body.starts_with("<!--convoy:escalation")), "{comments:?}");
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:account ")).count(),
        1,
        "escalating leaves one work account: {comments:?}"
    );
}

#[test]
fn blocked_parks_the_issue_and_it_returns_at_the_stage_that_parked_it() {
    let world = bound();
    world.issue(3, &["flow:feature"]);
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);
    tick(&world, 1180);
    declare(&world, 7, "review", "blocked", &["--text", "Needs #3.", "--blocked-by", "3"]);
    assert_eq!(did(&tick(&world, 1270)), ["advance 7 done"]);
    assert_eq!(world.edges(7), [3]);
    assert_eq!(world.claims(), [(3, "alpha".to_string())]);
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);

    world.close(3, "completed");
    assert_eq!(frontier(&world), [(7, "review".to_string())]);
    assert_eq!(did(&tick(&world, 1450)), ["dispatch 7 done"]);
    let started = workers(&world, &worktree(&world, 7));
    assert!(started[0].contains(" AFKCONVOY_STAGE='review' "), "{started:?}");
}

#[test]
fn with_a_lock_standing_only_the_lock_issue_is_started_at_its_stage() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature", "fleet-lock"]);
    // A lock is a flow's to carry like any issue: with no flow label it
    // stands, and nobody is started on it.
    world.issue(9, &["fleet-lock"]);
    assert_eq!(frontier(&world), [(8, "build".to_string())]);
    assert_eq!(excluded(&world, 7), "a fleet lock stands (#8, #9)");
    assert_eq!(excluded(&world, 9), "no flow label");
    let ticked = tick(&world, 1000);
    assert_eq!(did(&ticked), ["dispatch 8 done"]);
    assert_eq!(ticked["locks"], json!([8, 9]));

    world.close_issue(8);
    world.close_issue(9);
    let resumed = did(&tick(&world, 1090));
    assert!(resumed.contains(&"dispatch 7 done".to_string()), "{resumed:?}");
}

#[test]
fn already_satisfied_closes_the_issue() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "already-satisfied", &["--text", "main already does this."]);
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert_eq!(world.orca()["worktrees"], json!([]), "a worktree with no work goes");
}

#[test]
fn already_satisfied_closes_a_branch_only_behind_the_base() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    let dir = worktree(&world, 7);
    commit(&world.clone, "landed.txt", "landed while the stage checked\n");
    git(&world.clone, &["push", "-q", "origin", "main"]);
    declare(&world, 7, "build", "already-satisfied", &["--text", "main already does this."]);
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert!(!dir.exists());
}

#[test]
fn already_satisfied_refuses_an_equal_tree_with_commits_ahead() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    let dir = worktree(&world, 7);
    commit(&dir, "work.txt", "temporary work\n");
    git(&dir, &["revert", "--no-edit", "HEAD"]);
    declare(&world, 7, "build", "already-satisfied", &["--text", "main already does this."]);
    assert_eq!(did(&tick(&world, 1090)), ["advance 7 done"]);
    assert_eq!(world.state(7), "open");
    assert_eq!(world.claims(), []);
    assert!(dir.exists());
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    let escalation = world
        .comments(7)
        .into_iter()
        .find(|body| body.starts_with("<!--convoy:escalation"))
        .expect("an escalation comment");
    assert!(escalation.contains("commits past the merge base"), "{escalation}");
}

#[test]
fn a_worker_that_stops_with_no_verdict_keeps_its_stage_while_waiting_for_the_operator() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    let dir = worktree(&world, 7);
    stopped(&world, 1000);
    tick(&world, 1400);
    tick(&world, 1800);
    assert_eq!(workers(&world, &dir).len(), 1);
    assert_eq!(world.claims().len(), 1);
    assert!(!world.labels(7).contains(&"ready-for-human".to_string()));
    assert!(!world.comments(7).join("\n").contains("stopped with no valid verdict"));
}

#[test]
fn a_record_naming_a_stage_the_workflow_no_longer_has_escalates_the_issue() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);
    // Nobody holds it: its `review` worker is gone, with its claim.
    world.afk(&["release", "--issue", "7", "--instance", "alpha", "--host", "h1"]).ok();

    let team = world.root.join("github/acme/team");
    commit(&team, "workflows/feature.md", &FEATURE.replace("build", "make"));
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 1180)), ["escalate 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(said.contains("The workflow changed under this issue"), "{said}");
    assert!(said.contains("`build`"), "{said}");
    assert!(world.labels(7).contains(&"ready-for-human".to_string()));
    assert!(!world.labels(7).contains(&"flow:feature".to_string()));
    assert_eq!(world.claims(), []);
}

#[test]
fn the_next_stages_worker_starts_beside_the_frontier_with_no_bound_between_them() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    let first = tick(&world, 1000);
    assert_eq!(did(&first), ["dispatch 7 done", "dispatch 8 done"]);
    assert_eq!((&first["held"], &first["frontier_remaining"]), (&json!(2), &json!(0)));

    // A verdict starts its issue's next stage, and an issue new to the
    // frontier starts beside it.
    world.issue(5, &["flow:feature"]);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    let second = tick(&world, 1090);
    assert_eq!(did(&second), ["advance 7 done", "dispatch 7 done", "dispatch 5 done"]);
    assert_eq!((&second["held"], &second["frontier_remaining"]), (&json!(3), &json!(0)));
}

/// `FEATURE` under these breakers, its `build` naming a consult.
fn limited(max_rework: u64, max_consecutive_rework: u64) -> String {
    let limits = format!(
        "```yaml\nmax_rework: {max_rework}\nmax_consecutive_rework: {max_consecutive_rework}\n"
    );
    let (human, consult) =
        ("    escalate: { to: human }\n", "    escalate: { to: reviewer, do: rule }\n");
    FEATURE.replacen("```yaml\n", &limits, 1).replacen(human, consult, 1)
}

/// `FEATURE` under these breakers, its `build` naming no consult: a breaker
/// goes to a human (with one, see `consult::a_tripped_breaker_goes_to_its_consult_…`).
fn alone(max_rework: u64, max_consecutive_rework: u64) -> String {
    let limits = format!(
        "```yaml\nmax_rework: {max_rework}\nmax_consecutive_rework: {max_consecutive_rework}\n"
    );
    FEATURE.replacen("```yaml\n", &limits, 1)
}

/// Where issue 7 is and the rework it has had, as `show --issue` reads them
/// off its stage records.
fn shown(world: &World) -> Value {
    world.afk(&["show", "--issue", "7"]).ok()
}

/// Walks issue 7 through `verdicts`, a worker each: it declares the next
/// verdict, and a tick carries that out and starts the worker the issue's
/// position then calls for — a tick of its own starting the first, and the
/// one after a park. After each, the rework the tick acts on is what `rehearse` prints for
/// the same verdicts, and the issue is a human's exactly where `rehearse` says
/// a breaker trips. `blocked` is by #3, which closes once the issue is parked.
fn walk(world: &World, verdicts: &[&str]) -> Value {
    let args = [&["rehearse", "--team", "acme/team", "feature"][..], verdicts].concat();
    let rehearsed = world.afk(&args).ok();
    let mut now = 1000;
    for (nth, verdict) in verdicts.iter().enumerate() {
        if !world.claims().iter().any(|(issue, _)| *issue == 7) {
            assert_eq!(did(&tick(world, now)), ["dispatch 7 done"], "before verdict {nth}");
        }
        let stage = shown(world)["position"]["stage"].as_str().unwrap().to_string();
        let text = format!("Verdict {nth}: `{stage}` says {verdict}.");
        let blocked_by: &[&str] = if *verdict == "blocked" { &["--blocked-by", "3"] } else { &[] };
        declare(world, 7, &stage, verdict, &[&["--text", &text][..], blocked_by].concat());
        let carried = did(&tick(world, now + 90));
        let held = world.claims().iter().any(|(issue, _)| *issue == 7);
        let expected: &[&str] =
            if held { &["advance 7 done", "dispatch 7 done"] } else { &["advance 7 done"] };
        assert_eq!(carried, expected, "after verdict {nth}");
        if *verdict == "blocked" {
            world.close(3, "completed");
        }
        now += 180;

        let step = &rehearsed["steps"][nth];
        let escalated = world.labels(7).contains(&"ready-for-human".to_string());
        assert_eq!(escalated, !step["breaker"].is_null(), "after verdict {nth}: {step}");
        if escalated {
            let said = world.comments(7).join("\n");
            let breaker = step["breaker"].as_str().unwrap();
            assert!(said.contains(&format!("The breaker `{breaker}` tripped")), "{said}");
        } else {
            let read = shown(world);
            assert_eq!(read["rework"]["had"], step["rework"], "after verdict {nth}");
            assert_eq!(
                read["consecutive_rework"]["review"]["had"], step["consecutive_rework"]["review"],
                "after verdict {nth}"
            );
        }
    }
    rehearsed
}

/// The commands of every terminal ever opened.
fn started(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap().iter();
    terminals.filter_map(|t| t["command"].as_str().map(str::to_string)).collect()
}

#[test]
fn a_stage_that_rejects_too_often_in_a_row_is_escalated_with_every_round_listed() {
    let world = bound_to(&alone(3, 2));
    world.issue(7, &["flow:feature"]);
    let rehearsed = walk(&world, &["pass", "reject", "pass", "reject", "pass", "reject"]);
    assert_eq!(rehearsed["steps"][5]["breaker"], "max_consecutive_rework");

    let said = world.comments(7).join("\n");
    assert!(
        said.contains(
            "The breaker `max_consecutive_rework` tripped: `build` would have run again once \
             more than it allows.\n\nIts stage record: https://gh/c/"
        ),
        "{said}"
    );
    let rounds: Vec<usize> = [1, 3, 5]
        .iter()
        .enumerate()
        .map(|(round, verdict)| {
            let listed = format!("{}. `review` declared `reject` — https://gh/c/", round + 1);
            let reason = format!("> Verdict {verdict}: `review` says reject.");
            let at = said.find(&listed).unwrap_or_else(|| panic!("{listed} in {said}"));
            assert!(said[at..].contains(&reason), "{reason} after {listed} in {said}");
            at
        })
        .collect();
    assert!(rounds.is_sorted(), "oldest first: {said}");
    assert!(!said.contains("4. `"), "{said}");

    // It is a human's: no flow label, no claim, and nobody started on it.
    assert_eq!(world.labels(7), ["ready-for-human", "stage:build"]);
    assert_eq!(world.claims(), []);
    let before = started(&world).len();
    assert_eq!(frontier(&world), []);
    assert_eq!(did(&tick(&world, 5000)), [] as [&str; 0]);
    assert_eq!(did(&tick(&world, 5090)), [] as [&str; 0]);
    assert_eq!(started(&world).len(), before);
    assert!(started(&world).iter().all(|command| !command.contains("AFKCONVOY_SKILL='rule'")));
}

#[test]
fn an_issue_reworked_too_often_in_total_is_escalated_instead_of_dispatched() {
    let world = bound_to(&alone(2, 5));
    world.issue(7, &["flow:feature"]);
    let rehearsed = walk(&world, &["pass", "reject", "pass", "reject", "pass", "reject"]);
    assert_eq!(rehearsed["steps"][5]["breaker"], "max_rework");
    assert_eq!(rehearsed["steps"][5]["rework"], 2, "the rework it refuses is not counted");

    let said = world.comments(7).join("\n");
    for round in ["1. `review`", "2. `review`", "3. `review`"] {
        assert!(said.contains(round), "{round} in {said}");
    }
    assert_eq!(world.labels(7), ["ready-for-human", "stage:build"]);
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 5000)), [] as [&str; 0]);
}

#[test]
fn running_again_after_a_park_counts_as_no_rework() {
    let world = bound_to(&limited(2, 2));
    world.issue(3, &["flow:feature"]);
    world.issue(7, &["flow:feature"]);
    // Twice parked and twice rejected: the two rejections are all the rework
    // there is, and both breakers allow them.
    let verdicts = ["blocked", "pass", "blocked", "reject", "pass", "reject"];
    let rehearsed = walk(&world, &verdicts);
    assert!(rehearsed["steps"].as_array().unwrap().iter().all(|step| step["breaker"].is_null()));
    let read = shown(&world);
    assert_eq!(read["rework"], json!({"had": 2, "left": 0}));
    assert_eq!(read["position"]["stage"], "build");
    assert_eq!(world.labels(7), ["flow:feature", "stage:build"]);
    assert!(workers(&world, &worktree(&world, 7))[0].contains(" AFKCONVOY_STAGE='build' "));
}

#[test]
fn a_breaker_found_on_an_issue_nobody_holds_is_escalated_with_its_rounds_too() {
    let world = bound_to(&alone(0, 2));
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);
    tick(&world, 1090);
    declare(&world, 7, "review", "reject", &["--text", "Not yet."]);
    world.afk(&["release", "--issue", "7", "--instance", "alpha", "--host", "h1"]).ok();
    assert_eq!(excluded(&world, 7), "");

    assert_eq!(did(&tick(&world, 1270)), ["escalate 7 done"]);
    let said = world.comments(7).join("\n");
    assert!(said.contains("The breaker `max_rework` tripped"), "{said}");
    assert!(said.contains("1. `review` declared `reject` — https://gh/c/"), "{said}");
    assert!(said.contains("> Not yet."), "{said}");
    assert_eq!(world.claims(), []);
    assert_eq!(did(&tick(&world, 1360)), [] as [&str; 0]);
}

/// `run` as alpha at `now`, for `ticks` ticks → its lines, and how long it took.
fn run(world: &World, now: &str, ticks: &str, pacing: &[&str]) -> (Vec<String>, Duration) {
    let started = Instant::now();
    let args = [
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        now,
        "--ticks",
        ticks,
    ];
    let (lines, _) = world.afk_lines(&[&args[..], pacing].concat());
    (lines, started.elapsed())
}

#[test]
fn run_starts_the_stage_a_verdict_leads_to_in_the_tick_that_reads_it() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);

    // Nothing is left for the tick after it: there is nothing to hurry for.
    let (lines, _) = run(&world, "1090", "1", &["--second-ms", "0"]);
    assert_eq!(
        lines,
        [
            // the advance wakes the fleet, as after any verdict
            "woken during tick 1: tick 2 follows",
            "tick 1: advanced #7 → review · dispatched #7 · holding 1 · frontier 0 · next in 90s",
        ]
    );
    assert!(workers(&world, &worktree(&world, 7))[0].contains(" AFKCONVOY_STAGE='review' "));
    assert_eq!(did(&tick(&world, 1180)), [] as [&str; 0]);
}

#[test]
fn run_starts_what_a_verdict_left_the_next_tick_with_no_sleep() {
    // `build` names a consult: its `escalate` leaves a worker no stage's
    // verdict starts in the tick that reads it.
    let world = bound_to(&limited(3, 2));
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, 7, "build", "escalate", &["--text", "Which schema?"]);

    // Its seconds are real ones: a sleep between the two ticks is 90 of them.
    let (lines, took) = run(&world, "1090", "2", &[]);
    assert_eq!(
        lines,
        [
            "woken during tick 1: tick 2 follows",
            "tick 1: advanced #7 → build · holding 0 · frontier 0 · next now",
            "tick 2: dispatched #7 · holding 1 · frontier 0 · next in 90s",
        ]
    );
    assert!(took < Duration::from_secs(45), "the sleep was not skipped: {took:?}");
    assert!(workers(&world, &worktree(&world, 7))[0].contains(" AFKCONVOY_CONSULT='true' "));
}

#[test]
fn run_gives_the_slot_a_verdict_freed_to_the_stage_it_led_to() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, 7, "build", "pass", &["--text", "Built."]);

    // #8, never started, has no slot to start in.
    let (lines, _) = run(&world, "1090", "1", &["--second-ms", "0"]);
    assert_eq!(
        lines,
        [
            "woken during tick 1: tick 2 follows",
            "tick 1: advanced #7 → review · dispatched #7 · holding 1 · frontier 1 · next in 90s",
        ]
    );
}

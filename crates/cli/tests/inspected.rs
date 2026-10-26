//! `afkconvoy inspected`: an inspection, the operator on its host, says what
//! held the work back and what it did, from the checkout it was started in,
//! for its `run` to log and keep. And `afkconvoy land --inspection`: a change
//! of the repo it made itself lands through the landing train, on no issue.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{World, commit, git};
use serde_json::{Value, json};

/// A checkout of `world`'s repo named as an inspection's is.
fn inspection_checkout(world: &World) -> std::path::PathBuf {
    let checkout = world.root.join("wt").join("afkconvoy-inspection");
    let made = Command::new("git")
        .current_dir(&world.clone)
        .args(["worktree", "add", "-q", "--detach", checkout.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(made.success());
    checkout
}

/// What the inspection in `checkout` said, as its `run` reads it.
fn kept(checkout: &std::path::Path) -> serde_json::Value {
    let git_dir = Command::new("git")
        .current_dir(checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    let kept =
        std::fs::read_to_string(format!("{}/convoy-inspected.json", git_dir.trim())).unwrap();
    serde_json::from_str(&kept).unwrap()
}

#[test]
fn an_inspection_keeps_what_it_did_in_its_checkout_and_the_last_said_stands() {
    let world = World::new();
    let checkout = inspection_checkout(&world);
    let said =
        world.afk_in(&checkout, &["inspected", "--text", "a test never ends; it still hangs"]).ok();
    assert_eq!((said["inspected"].as_bool(), said["found"].as_str()), (Some(true), Some("said")));
    assert_eq!((said["kind"].as_str(), said["prs"].clone()), (Some("worker"), json!([])));
    std::fs::write(checkout.join("found.md"), "cargo build of 900 crates\n").unwrap();
    let said = world
        .afk_in(
            &checkout,
            &[
                "inspected",
                "--text-file",
                "found.md",
                "--did",
                "ended pid 4242, an orphaned cargo build",
                "--did",
                "told the worker to build through afkconvoy exclusive",
            ],
        )
        .ok();
    assert_eq!(said["found"], "said", "{said}");
    assert_eq!(said["did"].as_array().map(Vec::len), Some(2), "{said}");
    let kept = kept(&checkout);
    assert_eq!(kept["said"], "cargo build of 900 crates", "the last said stands: {kept}");
    assert_eq!(kept["did"][1], "told the worker to build through afkconvoy exclusive");
}

#[test]
fn a_host_inspection_says_what_it_did_in_the_same_words() {
    let world = World::new();
    let checkout = inspection_checkout(&world);
    let git_dir = Command::new("git")
        .current_dir(&checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    std::fs::write(format!("{}/convoy-inspection-kind", git_dir.trim()), "host").unwrap();
    let said = world
        .afk_in(
            &checkout,
            &["inspected", "--text", "five worktrees of 14 GiB", "--did", "removed their target"],
        )
        .ok();
    assert_eq!((said["found"].as_str(), said["kind"].as_str()), (Some("said"), Some("host")));
}

#[test]
fn only_an_inspection_s_checkout_says_what_was_done() {
    let world = World::new();
    let elsewhere = world.afk(&["inspected", "--text", "x"]).error();
    assert!(elsewhere.contains("is no inspection's checkout"), "{elsewhere}");
    let checkout = inspection_checkout(&world);
    let empty = world.afk_in(&checkout, &["inspected", "--text", " "]).error();
    assert!(empty.contains("the text is empty"), "{empty}");
    let did = world.afk_in(&checkout, &["inspected", "--text", "x", "--did", " "]).error();
    assert!(did.contains("a --did is empty"), "{did}");
    let found = world.afk_in(&checkout, &["inspected", "--found", "heavy", "--text", "x"]).error();
    assert!(found.contains("--found"), "no finding word is asked for any more: {found}");
    let left = world.afk_in(&checkout, &["inspected", "--text", "x", "--left", "y"]).error();
    assert!(left.contains("--left"), "what it could not fix goes in its text: {left}");
}

/// The branch orca names for an inspection's checkout.
const BRANCH: &str = "tester/afkconvoy-inspection";

/// A checkout of `world`'s repo named as an inspection's is, on a branch of
/// its own at the base branch's tip, as orca cuts one.
fn inspection_on_branch(world: &World) -> PathBuf {
    operator_on_branch(world, "afkconvoy-inspection")
}

fn operator_on_branch(world: &World, name: &str) -> PathBuf {
    git(&world.clone, &["fetch", "-q", "origin"]);
    let checkout = world.root.join("wt").join(name);
    let at = checkout.to_str().unwrap();
    git(&world.clone, &["worktree", "add", "-q", "-b", BRANCH, at, "origin/main"]);
    checkout
}

/// The inspection in `checkout` commits a change of the repo on its branch,
/// pushes it and opens its PR, #90, which closes no issue → its head.
fn change(world: &World, checkout: &Path) -> String {
    let head = commit(checkout, "gate.sh", "afkconvoy exclusive -- cargo test\n");
    git(checkout, &["push", "-q", "origin", "HEAD"]);
    world.with_gh(|gh| {
        let pr = json!({"number": 90, "title": "Queue the gates", "headRefName": BRANCH,
            "headRefOid": "", "body": "Run the gate through the host's queue.\n", "state": "open"});
        gh["prs"].as_array_mut().unwrap().push(pr);
    });
    head
}

/// The train's own worktree.
fn train(world: &World) -> PathBuf {
    let dir = world.root.join("train");
    git(&world.clone, &["fetch", "-q", "origin"]);
    let at = dir.to_str().unwrap();
    git(&world.clone, &["worktree", "add", "-q", "-b", "convoy-train", at, "origin/main"]);
    dir
}

/// The `gh` calls since the last look that write an issue: none of an
/// inspection's.
fn issue_writes(world: &World) -> Vec<Vec<String>> {
    let writes = ["create", "close", "edit", "comment", "reopen"];
    let calls = world.gh_calls().into_iter();
    calls.filter(|call| call[0] == "issue" && writes.contains(&call[1].as_str())).collect()
}

/// An inspection in `world` makes a change of the repo, gates it and puts it
/// on the landing train; the train lands it on the base branch, and no issue
/// is opened, closed or labelled for it.
fn lands_on_no_issue(world: &World, name: &str) {
    let checkout = operator_on_branch(world, name);
    let head = change(world, &checkout);
    let gated = world.afk_in(&checkout, &["gate", "--train"]).ok();
    assert_eq!(
        (gated["status"].as_str(), gated["recorded"].as_bool()),
        (Some("green"), Some(true))
    );
    world.gh_calls();

    let joined = world.afk_in(&checkout, &["land", "--inspection", "--host", "h1"]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");
    assert_eq!((joined["inspection"].as_str(), joined["pr"].as_u64()), (Some("h1"), Some(90)));
    let commit = joined["commit"].as_str().unwrap().to_string();
    let message = world.remote(&["log", "-1", "--format=%B", &commit]);
    assert_eq!(message, "Queue the gates (#90)\n\nBy an inspection on h1");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    assert_eq!(world.remote(&["log", "-1", "--format=%P", &commit]), format!("{base} {head}"));
    let again = world.afk_in(&checkout, &["land", "--inspection", "--host", "h1"]).ok();
    assert_eq!((again["outcome"].as_str(), again["again"].as_bool()), (Some("joined"), Some(true)));

    // A fleet with nothing of its own on the train tends it for the inspection's PR.
    let plan = world.afk(&["plan", "--instance", "me", "--now", "10000"]).ok();
    assert_eq!(plan["inspections_on_train"], json!([90]), "{plan}");
    // What the inspection says names the PR.
    if name.starts_with("afkconvoy-watch") {
        let said = world.afk_in(&checkout, &["inspected", "--text", "four gates at once"]).error();
        assert!(said.contains("is no inspection's checkout"), "a watch prints its record");
    } else {
        let said = world.afk_in(&checkout, &["inspected", "--text", "four gates at once"]).ok();
        assert_eq!(said["prs"], json!([90]), "{said}");
    }

    let train = train(world);
    let landed: Value = world.afk_in(&train, &["land", "--train", "--merged-timeout", "5"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    assert_eq!(landed["landed"], json!([{"inspection": "h1", "pr": 90, "commit": commit}]));
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), commit, "on the base branch");
    assert_eq!(landed["unmerged"], json!([]), "GitHub shows the PR merged");
    let branches = world.remote(&["for-each-ref", "--format=%(refname:short)", "refs/heads"]);
    assert_eq!(branches, "main", "the landed PR's branch is deleted");
    assert_eq!(issue_writes(world), [] as [Vec<String>; 0], "no issue of the change");
    assert_eq!(world.gh()["issues"], json!([]), "no issue was opened");
    let plan = world.afk(&["plan", "--instance", "me", "--now", "10100"]).ok();
    assert!(plan.get("inspections_on_train").is_none(), "landed, off the train: {plan}");
}

#[test]
fn an_inspection_s_change_lands_through_the_landing_train_on_no_issue() {
    let world = World::new();
    world.set_gate("true");
    lands_on_no_issue(&world, "afkconvoy-inspection");
}

#[test]
fn a_watch_s_change_is_gated_and_lands_on_the_inspection_path_on_no_issue() {
    let world = World::new();
    world.set_gate("true");
    // A watch's numeric suffix is accepted, as an inspection's is.
    lands_on_no_issue(&world, "afkconvoy-watch-79");
}

#[test]
fn an_inspection_s_change_lands_in_a_bound_repo_with_no_intake_nor_flow_label() {
    let team = "```yaml\nroles:\n  dev: acme/dev\n```\n";
    let feature = "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    \
                   on: { pass: done }\n```\n";
    let skill = "---\nname: implement\ndescription: build\n---\n";
    for name in ["afkconvoy-inspection", "afkconvoy-watch"] {
        let world = World::new();
        world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", skill)]);
        world.repo("acme/team", &[("team.md", team), ("workflows/feature.md", feature)]);
        world.set_gate_config("gate_command: \"true\"\ngate_seconds: 600\nteam: acme/team");
        lands_on_no_issue(&world, name);
    }
}

#[test]
fn only_an_inspection_lands_by_the_inspection_and_only_its_branch_s_pr() {
    let world = World::new();
    world.set_gate("true");
    let elsewhere = world.afk(&["land", "--inspection"]).error();
    assert!(elsewhere.contains("is no inspection's checkout"), "{elsewhere}");
    let checkout = inspection_on_branch(&world);
    commit(&checkout, "gate.sh", "x\n");
    let unopened = world.afk_in(&checkout, &["land", "--inspection"]).error();
    assert!(unopened.contains("no open PR has this checkout's branch"), "{unopened}");
    assert!(unopened.contains("gh pr create --base main --head tester/afkconvoy-inspection"));
    let both = world.afk_in(&checkout, &["land", "--inspection", "--issue", "7"]).error();
    assert!(both.contains("cannot be used with"), "{both}");
}

#[test]
fn with_a_fleet_lock_standing_an_inspection_s_pr_joins_nothing() {
    let world = World::new();
    world.set_gate("true");
    world.issue(5, &["fleet-lock"]);
    let checkout = inspection_on_branch(&world);
    change(&world, &checkout);
    let locked = world.afk_in(&checkout, &["land", "--inspection", "--host", "h1"]).ok();
    assert_eq!(locked["outcome"], "locked", "{locked}");
    assert_eq!(locked["locks"], json!([5]));
    let line = world.remote(&["for-each-ref", "--format=%(objectname)", "refs/convoy/train"]);
    assert_eq!(line, "", "nothing joined");
    let said = world.afk_in(&checkout, &["inspected", "--text", "a lock stands"]).ok();
    assert_eq!(said["prs"], json!([]), "{said}");
}

//! A watch's change lands on no issue, through its own turn.
use crate::common::{World, commit, git};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The branch orca names for a watch's checkout.
const BRANCH: &str = "tester/afkconvoy-watch";

/// A checkout of `world`'s repo named as a watch's is, on a branch of
/// its own at the base branch's tip, as orca cuts one.
fn watch_on_branch(world: &World) -> PathBuf {
    operator_on_branch(world, "afkconvoy-watch")
}

fn operator_on_branch(world: &World, name: &str) -> PathBuf {
    git(&world.clone, &["fetch", "-q", "origin"]);
    let checkout = world.root.join("wt").join(name);
    let at = checkout.to_str().unwrap();
    git(&world.clone, &["worktree", "add", "-q", "-b", BRANCH, at, "origin/main"]);
    checkout
}

/// The watch in `checkout` commits a change of the repo on its branch,
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

/// The `gh` calls since the last look that write an issue: none of an
/// watch's.
fn issue_writes(world: &World) -> Vec<Vec<String>> {
    let writes = ["create", "close", "edit", "comment", "reopen"];
    let calls = world.gh_calls().into_iter();
    calls.filter(|call| call[0] == "issue" && writes.contains(&call[1].as_str())).collect()
}

/// A watch in `world` makes a change and lands its merge in a gated turn.
/// No issue
/// is opened, closed or labelled for it.
fn lands_on_no_issue(world: &World, name: &str) {
    let checkout = operator_on_branch(world, name);
    let head = change(world, &checkout);
    world.gh_calls();

    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let joined = world.afk_in(&checkout, &["land", "--watch", "--host", "h1"]).ok();
    assert_eq!(joined["status"], "green");
    assert_eq!(joined["recorded"], true);
    assert_eq!(joined["outcome"], "merged", "{joined}");
    assert_eq!((joined["watch"].as_str(), joined["pr"].as_u64()), (Some("h1"), Some(90)));
    let commit = joined["commit"].as_str().unwrap().to_string();
    let message = world.remote(&["log", "-1", "--format=%B", &commit]);
    assert_eq!(message, "Queue the gates (#90)\n\nBy a watch on h1");
    assert_eq!(world.remote(&["log", "-1", "--format=%P", &commit]), format!("{base} {head}"));
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), commit, "on the base branch");
    assert!(world.remote(&["for-each-ref", "refs/convoy/train"]).is_empty());
    assert_eq!(issue_writes(world), [] as [Vec<String>; 0], "no issue of the change");
    assert_eq!(world.gh()["issues"], json!([]), "no issue was opened");
    let plan = world.afk(&["plan", "--instance", "me", "--now", "10100"]).ok();
    assert!(plan.get("watches_on_train").is_none(), "no train: {plan}");
    crate::common::stopped_candidates_gone(world, &checkout);
}

#[test]
fn a_watch_s_change_is_gated_and_lands_on_the_watch_path_on_no_issue() {
    let world = World::new();
    world.set_gate("true");
    // A watch's numeric suffix is accepted, as a watch's is.
    lands_on_no_issue(&world, "afkconvoy-watch-79");
}

#[test]
fn a_watch_s_change_lands_in_a_bound_repo_with_no_intake_nor_flow_label() {
    let team = "```yaml\nroles:\n  dev: acme/dev\n```\n";
    let feature = "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    \
                   on: { pass: done }\n```\n";
    let skill = "---\nname: implement\ndescription: build\n---\n";
    for name in ["afkconvoy-watch", "afkconvoy-watch-95"] {
        let world = World::new();
        world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", skill)]);
        world.repo("acme/team", &[("team.md", team), ("workflows/feature.md", feature)]);
        world.set_gate_config("gate_command: \"true\"\ngate_seconds: 600\nteam: acme/team");
        lands_on_no_issue(&world, name);
    }
}

#[test]
fn only_a_watch_lands_by_the_watch_and_only_its_branch_s_pr() {
    let world = World::new();
    world.set_gate("true");
    let elsewhere = world.afk(&["land", "--watch"]).error();
    assert!(elsewhere.contains("is no watch's checkout"), "{elsewhere}");
    let checkout = watch_on_branch(&world);
    commit(&checkout, "gate.sh", "x\n");
    let unopened = world.afk_in(&checkout, &["land", "--watch"]).error();
    assert!(unopened.contains("no open PR has this checkout's branch"), "{unopened}");
    assert!(unopened.contains("gh pr create --base main --head tester/afkconvoy-watch"));
    let both = world.afk_in(&checkout, &["land", "--watch", "--issue", "7"]).error();
    assert!(both.contains("cannot be used with"), "{both}");
}

#[test]
fn with_a_fleet_lock_standing_a_watch_s_pr_joins_nothing() {
    let world = World::new();
    world.set_gate("true");
    world.issue(5, &["fleet-lock"]);
    let checkout = watch_on_branch(&world);
    change(&world, &checkout);
    let locked = world.afk_in(&checkout, &["land", "--watch", "--host", "h1"]).ok();
    assert_eq!(locked["outcome"], "locked", "{locked}");
    assert_eq!(locked["locks"], json!([5]));
    let line = world.remote(&["for-each-ref", "--format=%(objectname)", "refs/convoy/train"]);
    assert_eq!(line, "", "nothing joined");
}

#[test]
fn a_watch_can_land_with_graphql_unavailable_and_still_obeys_locks() {
    for locked in [false, true] {
        let world = World::new();
        world.set_gate("true");
        let checkout = watch_on_branch(&world);
        change(&world, &checkout);
        if locked {
            world.issue(5, &["fleet-lock"]);
        }
        world.with_gh(|gh| gh["fail"] = json!(["api graphql", "pr list"]));
        let base = world.remote(&["rev-parse", "refs/heads/main"]);
        let answer = world.afk_in(&checkout, &["land", "--watch", "--host", "h1"]).ok();
        if locked {
            assert_eq!(answer["outcome"], "locked", "{answer}");
            assert_eq!(answer["locks"], json!([5]));
            assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
        } else {
            assert_eq!(answer["outcome"], "merged", "{answer}");
            assert_eq!(answer["status"], "green");
            assert_eq!(answer["recorded"], true);
            assert!(issue_writes(&world).is_empty());
        }
    }
}

#[test]
fn a_watch_never_treats_a_failed_rest_lock_read_as_no_locks() {
    let world = World::new();
    world.set_gate("true");
    let checkout = watch_on_branch(&world);
    change(&world, &checkout);
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    world.with_gh(|gh| gh["fail"] = json!(["issues?state=open"]));
    let error = world.afk_in(&checkout, &["land", "--watch"]).error();
    assert!(error.contains("injected failure"), "{error}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
}

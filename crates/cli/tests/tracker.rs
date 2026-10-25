//! A fleet whose tracker is another repo than its checkout's: every issue it
//! reads and writes is the tracker's, and every PR, branch and ref the
//! checkout's repo's.

use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TRACKER: &str = "acme/tracker";
/// The repo label of the checkout's repo, `acme/dev`.
const MINE: &str = "repo:acme/dev";

/// An unbound repo, `acme/dev`, whose checkout says `acme/tracker` is its
/// tracker, and whose gate passes.
fn world() -> World {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, "acme/dev");
    world.say_tracker(&world.clone, TRACKER);
    world
}

/// Adds an open issue carrying `labels` to the tracker.
fn tracker_issue(world: &World, number: u64, labels: &[&str]) {
    issue_in(world, TRACKER, number, labels);
}

/// Adds an open issue carrying `labels` to the tracker `tracker`.
fn issue_in(world: &World, tracker: &str, number: u64, labels: &[&str]) {
    world.with_tracker_of(tracker, |gh| {
        let issue = json!({"number": number, "title": format!("tracked {number}"),
            "labels": labels, "state": "open"});
        gh["issues"].as_array_mut().unwrap().push(issue);
    });
}

fn issue_of(state: &Value, number: u64) -> Value {
    let rows = state["issues"].as_array().unwrap();
    rows.iter().find(|row| row["number"] == number).unwrap().clone()
}

fn comments_of(state: &Value, number: u64) -> Vec<String> {
    let rows = state["comments"][number.to_string()].as_array().cloned().unwrap_or_default();
    rows.iter().map(|row| row["body"].as_str().unwrap().to_string()).collect()
}

fn tick(world: &World, now: &str) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]).ok()
}

/// The brief of the worker in `worktree`.
fn brief(worktree: &Path) -> String {
    let git_dir = PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let steps =
        steps.filter(|step| !["board", "heartbeat"].contains(&step["do"].as_str().unwrap()));
    let steps = steps.filter(|step| step["outcome"] != "nothing");
    steps
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// The train's own worktree.
fn train(world: &World) -> PathBuf {
    let dir = world.root.join("train");
    git(&world.clone, &["fetch", "-q", "origin"]);
    let at = dir.to_str().unwrap();
    git(&world.clone, &["worktree", "add", "-q", "-b", "convoy-train", at, "origin/main"]);
    dir
}

/// The repo a call names, and what of it: an endpoint's path after the repo.
/// The tracker is named by name, and the checkout's repo by `{owner}/{repo}`,
/// which `gh` fills in from the checkout: a query names its repo by its
/// `owner` and `repo`, and `gh pr` the checkout's by naming none.
fn named(call: &[String]) -> (String, String) {
    let words: Vec<&str> = call.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["pr", verb, ..] => {
            assert!(!call.contains(&"--repo".to_string()), "{call:?}");
            return ("{owner}/{repo}".to_string(), format!("pr {verb}"));
        }
        ["repo", "clone", repo, ..] => return (repo.to_string(), "clone".to_string()),
        ["api", "graphql", ..] => {
            let field = |name: &str| {
                let prefix = format!("{name}=");
                call.iter().find_map(|arg| arg.strip_prefix(&prefix)).unwrap().to_string()
            };
            return (format!("{}/{}", field("owner"), field("repo")), "graphql".to_string());
        }
        _ => {}
    }
    let endpoint = call.iter().find_map(|arg| arg.strip_prefix("repos/")).expect("an endpoint");
    let parts: Vec<&str> = endpoint.splitn(3, '/').collect();
    (format!("{}/{}", parts[0], parts[1]), parts[2].to_string())
}

/// Every `gh` call made of `world`'s checkout and its worktrees → those of
/// them made of the tracker, once it is shown that each is of an issue or a
/// label, and was answered from the tracker's state; that each made of the
/// checkout's repo is of a PR — its comments among them — or a branch; and
/// that each made of another repo reads a team's or a role's head.
fn routed(world: &World) -> Vec<Vec<String>> {
    let gh = world.gh();
    let prs: Vec<String> =
        gh["prs"].as_array().unwrap().iter().map(|pr| pr["number"].to_string()).collect();
    let on_prs: Vec<&Value> =
        prs.iter().flat_map(|pr| gh["comments"][pr].as_array()).flatten().collect();
    let calls: Vec<Vec<String>> = serde_json::from_value(gh["calls"].clone()).unwrap();
    let mut of_tracker = vec![];
    for call in &calls {
        let (repo, what) = named(call);
        match repo.as_str() {
            TRACKER => {
                let issues = what.starts_with("issues") || what.starts_with("labels");
                assert!(issues || what == "graphql", "{call:?}");
                of_tracker.push(call.clone());
            }
            "{owner}/{repo}" => {
                let rest = what.strip_prefix("issues/");
                let on = rest.and_then(|rest| rest.split('/').next()).unwrap_or_default();
                let of_pr = what.starts_with("pr ")
                    || what.starts_with("branches/")
                    || prs.iter().any(|pr| pr == on)
                    || rest.and_then(|rest| rest.strip_prefix("comments/")).is_some_and(|id| {
                        on_prs.iter().any(|comment| comment["id"].as_u64() == id.parse().ok())
                    });
                assert!(of_pr, "a call of the checkout's repo is of a PR or a branch: {call:?}");
            }
            other => {
                assert!(!gh["repos"][other].is_null(), "a call of an unknown repo: {call:?}");
                assert!(["clone", "commits/HEAD"].contains(&what.as_str()), "{call:?}");
            }
        }
    }
    let answered: Vec<Vec<String>> =
        serde_json::from_value(world.tracker()["calls"].clone()).unwrap();
    assert_eq!(answered, of_tracker, "the tracker answered each call made of it");
    of_tracker
}

#[test]
fn every_issue_is_the_trackers_and_every_pr_branch_and_ref_the_checkouts() {
    let world = world();
    tracker_issue(&world, 7, &["ready-for-agent", MINE]);
    // The checkout's repo has an issue of the same number, ready too: it is
    // not the fleet's, and nothing touches it.
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    let own = world.gh()["issues"].clone();

    assert_eq!(did(&tick(&world, "1000")), ["dispatch 7 done"]);
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    let claims = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claim/"]);
    assert_eq!(claims, "refs/convoy/claim/acme/tracker/7", "named for the tracker's issue");
    let tracker = world.tracker();
    let said = comments_of(&tracker, 7).join("\n");
    assert!(said.contains("afkconvoy works this issue on the branch"), "{said}");

    // Its worker: on a branch named for the tracker's issue, briefed to read
    // and write its issue in the tracker and to close it by the name the
    // repo gives it.
    let worktree = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    let branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(branch, "tester/acme-tracker-7-tracked-7");
    let brief = brief(&worktree);
    for said in [
        "**Your issue:** acme/tracker#7 — tracked 7",
        "`gh issue view 7 --repo acme/tracker --comments`",
        "`gh issue comment 7 --repo acme/tracker --body",
        "   Closes acme/tracker#7\"",
        "`gh issue create --repo acme/tracker`",
        "(`--label ready-for-agent`), and the repo label `repo:acme/dev` (`--label repo:acme/dev`)",
    ] {
        assert!(brief.contains(said), "{said}: {brief}");
    }
    // A commit pushed on the branch the claim named, a PR in the checkout's
    // repo, the gate, and a join.
    commit(&worktree, "seven.txt", "7\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);
    world.pr_closing(70, "acme/tracker#7", &branch);
    let gated = world.afk_in(&worktree, &["gate", "--train"]).ok();
    assert_eq!(
        (gated["status"].as_str(), gated["recorded"].as_bool()),
        (Some("green"), Some(true))
    );
    let joined = world.afk_in(&worktree, &["land", "--issue", "7", "--now", "1050"]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");

    // The train lands the PR and closes the issue: the tracker's.
    let landed = world.afk_in(&train(&world), &["land", "--train", "--merged-timeout", "5"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    assert_eq!(issue_of(&world.tracker(), 7)["state"], "closed");
    assert_eq!(did(&tick(&world, "1090")), ["settle 7 done"]);
    assert_eq!(world.claims(), []);
    let file = format!("{}:seven.txt", world.remote(&["rev-parse", "refs/heads/main"]));
    assert_eq!(world.remote(&["show", &file]), "7");
    let joined = world.remote(&["log", "--format=%s%n%b", "--merges", "-1", "main"]);
    assert_eq!(joined, "pr 70 (#70)\nCloses acme/tracker#7", "the join names the tracker's issue");

    // The checkout's repo's issues are as they were, with no comment.
    assert_eq!(world.gh()["issues"], own);
    let commented: Vec<String> =
        world.gh()["comments"].as_object().unwrap().keys().cloned().collect();
    let prs: Vec<String> =
        world.gh()["prs"].as_array().unwrap().iter().map(|pr| pr["number"].to_string()).collect();
    assert!(commented.iter().all(|number| prs.contains(number)), "{commented:?}");

    let of_tracker = routed(&world);
    for read in ["graphql", "issues/7", "issues/7/comments?per_page=100"] {
        assert!(of_tracker.iter().any(|call| named(call).1 == read), "{read}: {of_tracker:?}");
    }
    let closed = ["api", "--method", "PATCH", "repos/acme/tracker/issues/7", "-f", "state=closed"];
    assert!(of_tracker.iter().any(|call| call.starts_with(&closed.map(String::from))));
}

/// The `gh` calls a plan of `world` makes, each as its first two words, sorted.
fn plan_asks(world: &World) -> Vec<String> {
    world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    world.gh_calls();
    world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    let mut asked: Vec<String> = world.gh_calls().iter().map(|call| common::asked(call)).collect();
    asked.sort();
    asked
}

#[test]
fn a_rebuild_asks_github_as_often_of_another_tracker_as_of_its_own_repo() {
    let own = World::new();
    own.issue(7, &["ready-for-agent"]);
    let tracked = world();
    tracker_issue(&tracked, 7, &["ready-for-agent", MINE]);
    // However many issues of other repos the tracker holds, each with more
    // comments than a rebuild reads at once: none is read.
    for number in 20..40 {
        tracker_issue(&tracked, number, &["ready-for-agent", "repo:acme/web"]);
        tracked.with_tracker(|gh| {
            let many: Vec<Value> = (0..=afkconvoy::gh::COMMENTS_WITH as u64)
                .map(|id| {
                    let id = number * 1000 + id;
                    json!({"id": id, "body": "busy", "html_url": format!("https://gh/c/{id}")})
                })
                .collect();
            gh["comments"][number.to_string()] = json!(many);
        });
    }
    assert_eq!(plan_asks(&tracked), plan_asks(&own));
}

#[test]
fn a_tracker_issue_of_another_repo_or_of_none_is_not_the_fleets_and_one_of_two_repos_is_barred() {
    let world = world();
    tracker_issue(&world, 7, &["ready-for-agent", "Repo:Acme/Dev"]);
    tracker_issue(&world, 8, &["ready-for-agent", "repo:acme/web"]);
    tracker_issue(&world, 9, &["ready-for-agent"]);
    tracker_issue(&world, 10, &["ready-for-agent", MINE, "repo:acme/web"]);
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    let frontier: Vec<u64> = plan["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["number"].as_u64().unwrap())
        .collect();
    assert_eq!(frontier, [7], "{plan}");
    let excluded: Vec<(u64, &str)> = plan["excluded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["number"].as_u64().unwrap(), row["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        excluded,
        [(10, "repo labels of more than one repo (repo:acme/dev, repo:acme/web)")]
    );
    assert_eq!(plan["tracker"], TRACKER);
    // The rebuild asks the tracker for the repo's issues alone.
    let asked = world.gh_calls();
    let query = asked.iter().find(|call| call.starts_with(&["api".into(), "graphql".into()]));
    assert!(query.unwrap().contains(&format!("labels[]={MINE}")), "{query:?}");
}

#[test]
fn a_lock_issue_of_the_tracker_holds_the_fleet_and_one_of_the_checkouts_repo_does_not() {
    let world = world();
    tracker_issue(&world, 7, &["ready-for-agent", MINE]);
    world.issue(9, &["fleet-lock"]);
    // nor one of the tracker's that is another repo's
    tracker_issue(&world, 8, &["fleet-lock", "repo:acme/web"]);
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    let frontier: Vec<u64> = plan["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["number"].as_u64().unwrap())
        .collect();
    assert_eq!(frontier, [7], "{plan}");

    tracker_issue(&world, 9, &["fleet-lock", MINE]);
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"], json!([]), "{plan}");
}

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
    on: { pass: done }
```
";

/// `world`, its repo bound to a team with the one workflow `feature`.
fn bound() -> World {
    let world = world();
    let skill = |name: &str| format!("---\nname: {name}\ndescription: {name}\n---\n");
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", &skill("implement"))]);
    world.repo("acme/reviewer", &[(".claude/skills/review/SKILL.md", &skill("review"))]);
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// One tick of a bound repo at `now`, every worker started being at work.
fn tick_at_work(world: &World, now: u64) -> Value {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
                terminal["lastOutputAt"] = json!(now * 1000);
            }
        }
    });
    tick(world, &now.to_string())
}

#[test]
fn a_bound_repos_stage_records_labels_and_escalations_are_the_trackers() {
    let world = bound();
    tracker_issue(&world, 7, &["flow:feature", MINE]);
    tracker_issue(&world, 9, &["flow:chore", "bug", MINE]);
    world.issue(7, &["flow:feature"]);
    world.issue(9, &["flow:chore"]);
    let own = world.gh()["issues"].clone();

    let ticked = tick_at_work(&world, 1000);
    assert_eq!(did(&ticked), ["escalate 9 done", "dispatch 7 done", "stage_label 7 done"]);
    let tracker = world.tracker();
    assert_eq!(issue_of(&tracker, 9)["labels"], json!(["bug", MINE, "ready-for-human"]));
    let said = comments_of(&tracker, 9).join("\n");
    assert!(said.contains("flow:chore names no workflow of the team"), "{said}");

    let line = ["verdict", "pass", "--issue", "7", "--stage", "build", "--text", "Built."];
    let declared =
        world.afk(&[&line[..], &["--role-commit", "r1", "--team-commit", "t1"]].concat());
    assert_eq!(declared.ok()["declared"], true);
    let said = comments_of(&world.tracker(), 7).join("\n");
    assert!(said.contains("Built."), "the stage record is on the tracker's issue: {said}");
    let ticked = tick_at_work(&world, 1090);
    assert_eq!(
        did(&ticked),
        ["advance 7 done", "dispatch 7 done", "stage_label 7 done"],
        "{ticked}"
    );
    assert_eq!(
        issue_of(&world.tracker(), 7)["labels"],
        json!(["flow:feature", MINE, "stage:review"])
    );

    assert_eq!(world.gh()["issues"], own);
    assert_eq!(world.gh()["comments"], json!({}), "nothing is said on the checkout's repo");
    let of_tracker = routed(&world);
    let wrote: Vec<String> = of_tracker
        .iter()
        .filter(|call| call.contains(&"--method".to_string()))
        .map(|call| named(call).1)
        .collect();
    for written in ["issues/7/comments", "issues/9/comments", "issues/9/labels"] {
        assert!(wrote.iter().any(|what| what.starts_with(written)), "{written}: {wrote:?}");
    }
}

#[test]
fn two_fleets_of_one_repo_working_two_trackers_each_claim_land_and_close_only_their_own_issue() {
    const OTHER: &str = "acme/other";
    let world = world();
    let other = world.clone_again("other");
    world.name_origin(&other, "acme/dev");
    world.say_tracker(&other, OTHER);
    world.with_orca(|orca| {
        orca["repos"].as_array_mut().unwrap().push(json!({"id": "repo-2", "path": other}));
    });
    for tracker in [TRACKER, OTHER] {
        issue_in(&world, tracker, 7, &["ready-for-agent", MINE]);
    }
    world.issue(7, &["ready-for-agent"]);
    let own = world.gh()["issues"].clone();
    let tick_of = |dir: &Path, instance: &str, now: &str| {
        world.afk_in(dir, &["tick", "--instance", instance, "--host", "h1", "--now", now]).ok()
    };

    // Each claims its own tracker's #7: neither is the other's.
    assert_eq!(did(&tick_of(&world.clone, "alpha", "1000")), ["dispatch 7 done"]);
    assert_eq!(did(&tick_of(&other, "beta", "1000")), ["dispatch 7 done"]);
    let claims = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claim/"]);
    assert_eq!(claims, "refs/convoy/claim/acme/other/7\nrefs/convoy/claim/acme/tracker/7");
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);

    // Each worker opens its PR in the one repo and joins the one train.
    let worktrees = world.orca()["worktrees"].as_array().unwrap().clone();
    for (row, (closes, file)) in
        worktrees.iter().zip([("acme/tracker#7", "a"), ("acme/other#7", "b")])
    {
        let worktree = PathBuf::from(row["path"].as_str().unwrap());
        let branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);
        commit(&worktree, &format!("{file}.txt"), "7\n");
        git(&worktree, &["push", "-q", "origin", "HEAD"]);
        let pr = if file == "a" { 70 } else { 71 };
        world.pr_closing(pr, closes, &branch);
        let gated = world.afk_in(&worktree, &["gate", "--train"]).ok();
        assert_eq!(gated["status"], "green", "{gated}");
        let joined = world.afk_in(&worktree, &["land", "--issue", "7", "--now", "1050"]).ok();
        assert_eq!((&joined["outcome"], &joined["pr"]), (&json!("joined"), &json!(pr)), "{joined}");
    }

    // The train lands both; each fleet closes its own tracker's #7 alone.
    let landed = world.afk_in(&train(&world), &["land", "--train", "--merged-timeout", "5"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    assert_eq!(issue_of(&world.tracker_of(TRACKER), 7)["state"], "closed");
    assert_eq!(issue_of(&world.tracker_of(OTHER), 7)["state"], "open", "the other fleet's");
    assert_eq!(did(&tick_of(&world.clone, "alpha", "1090")), ["settle 7 done"]);
    let settled = tick_of(&other, "beta", "1090");
    assert_eq!(did(&settled), ["settle 7 done"], "{settled}");
    assert_eq!(issue_of(&world.tracker_of(OTHER), 7)["state"], "closed");
    assert_eq!(world.remote(&["for-each-ref", "refs/convoy/claim/"]), "");
    assert_eq!(world.gh()["issues"], own, "the repo's own #7 is as it was");
}

const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "100"];

#[test]
fn run_tracker_keeps_it_on_the_checkout_list_makes_its_labels_and_refuses_a_change_under_a_claim() {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, "acme/dev");
    world.put_tracker(TRACKER);
    world.issue(8, &["ready-for-agent"]);

    let first = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).ok();
    assert_eq!(first["tracker"], json!({"from": null, "to": TRACKER}), "{first}");
    assert!(first["first_tick"].as_str().unwrap().contains("frontier 0"), "{first}");
    assert_eq!(world.checkout_list()["checkouts"][0]["tracker"], TRACKER);
    let made = world.tracker()["labels"].clone();
    assert_eq!(made, json!(["ready-for-agent", MINE, "ready-for-human", "fleet-lock"]));

    // its next tick reads the tracker: its issue is dispatched, the repo's is not
    tracker_issue(&world, 7, &["ready-for-agent", MINE]);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while world.claims().is_empty() {
        assert!(std::time::Instant::now() < until, "no tick dispatched #7");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let instance = first["instance"].as_str().unwrap().to_string();
    assert_eq!(world.claims(), [(7, instance)]);

    // a `run` that names none leaves it as it is
    let again = world.afk(&RUN).ok();
    assert_eq!(again["tracker"], Value::Null, "{again}");
    assert_eq!(world.checkout_list()["checkouts"][0]["tracker"], TRACKER);

    // `plan`, `doctor` and `checkouts` say it
    assert_eq!(world.afk(&["plan"]).ok()["tracker"], TRACKER);
    assert_eq!(world.afk(&["doctor", "--host", "h1"]).json["fleet"]["tracker"], TRACKER);
    assert_eq!(world.afk(&["checkouts"]).ok()["checkouts"][0]["tracker"], TRACKER);

    // its fleet holds the claim on the tracker's #7: no change while it does
    let refused = world.afk(&[&RUN[..], &["--tracker", "default"]].concat()).error();
    assert!(refused.contains("works the issues of acme/tracker still"), "{refused}");
    assert!(refused.contains("it holds the claims on acme/tracker#7"), "{refused}");
    assert_eq!(world.checkout_list()["checkouts"][0]["tracker"], TRACKER);

    // naming no repo is refused, saying what is taken
    let error = world.afk(&["run", "--tracker", "acme"]).error();
    assert!(error.contains("`acme` is no repo, `<owner>/<repo>`, nor `default`"), "{error}");
}

//! A fleet whose tracker is another repo than its checkout's: every issue it
//! reads and writes is the tracker's, and every PR, branch and ref the
//! checkout's repo's, apart from the proposals read on the team's repo.

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

#[test]
fn target_selection_keeps_tracker_identity_and_refuses_foreign_ownership() {
    let world = world();
    tracker_issue(&world, 7, &[MINE, "ready-for-agent"]);
    tracker_issue(&world, 8, &["repo:acme/other", "ready-for-agent"]);
    git(&world.clone, &["push", "-q", "origin", "HEAD:delivery/tracker"]);
    world.afk(&["target", "--issue", "7", "--branch", "delivery/tracker"]).ok();
    let record: Value = serde_json::from_str(&world.remote(&[
        "show",
        "-s",
        "--format=%s",
        "refs/convoy/target/acme/tracker/7",
    ]))
    .unwrap();
    assert_eq!(record["issue"], "acme/tracker#7");
    assert_eq!(record["branch"], "delivery/tracker");
    // Another host may spell the same GitHub tracker with different case.
    let differently_spelled = json!({"issue": "ACME/TRACKER#7", "branch": "delivery/tracker"});
    let sha = afkconvoy::git::Repo::new(world.clone.clone())
        .record_commit(&differently_spelled.to_string())
        .unwrap();
    git(
        &world.clone,
        &["push", "-q", "--force", "origin", &format!("{sha}:refs/convoy/target/acme/tracker/7")],
    );
    assert_eq!(world.afk(&["target", "--issue", "7"]).ok()["target_branch"], "delivery/tracker");
    let error = world.afk(&["target", "--issue", "8", "--branch", "delivery/tracker"]).error();
    assert!(error.contains("another repo"), "{error}");
}

#[test]
fn unworked_blockers_are_named_with_repairs_in_plan_and_doctor() {
    let world = world();
    tracker_issue(&world, 7, &[MINE, "ready-for-agent"]);
    world.with_tracker(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 931, "state": "open", "repo": "acme/kernel", "labels": []},
            {"number": 41, "state": "open", "repo": TRACKER, "labels": []},
            {"number": 42, "state": "open", "repo": TRACKER, "labels": ["repo:acme/absent"]},
            {"number": 43, "state": "closed", "repo": "acme/kernel", "labels": []}
        ])
    });
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["frontier"], json!([]), "a build still waits: {plan}");
    let findings = plan["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 3, "{plan}");
    let external = findings
        .iter()
        .find(|row| row["what"].as_str().unwrap().contains("acme/kernel#931"))
        .unwrap();
    for said in ["issue #7", "no fleet works"] {
        assert!(external["what"].as_str().unwrap().contains(said), "{external}");
    }
    assert!(
        external["fix"]
            .as_str()
            .unwrap()
            .contains("gh issue transfer 931 'acme/tracker' --repo 'acme/kernel'"),
        "{external}"
    );
    for row in findings {
        let fix = row["fix"].as_str().unwrap();
        assert!(fix.contains("afkconvoy flows"), "{row}");
        assert!(fix.contains("gh issue edit ") && fix.contains("--add-label 'repo:"), "{row}");
    }
    let doctor = world.afk(&["doctor"]).ok();
    for finding in findings {
        assert!(doctor["findings"].as_array().unwrap().contains(finding), "{doctor}");
    }
}

#[test]
fn a_blocker_repair_read_in_a_workers_worktree_uses_the_main_checkout_for_flows() {
    let world = world();
    tracker_issue(&world, 7, &[MINE, "ready-for-agent"]);
    assert_eq!(did(&tick(&world, "1000")), ["dispatch 7 done"]);
    let worktree = world.worktrees()[0].clone();
    world.with_tracker(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 931, "state": "open", "repo": "acme/dev", "labels": []}
        ])
    });
    let plan = world.afk_in(&worktree, &["plan", "--now", "1090"]).ok();
    let fix = plan["findings"][0]["fix"].as_str().unwrap();
    let flows = fix.lines().find(|line| line.contains("afkconvoy flows")).unwrap();
    // The suggested shell command exercises this build: the world's
    // `afkconvoy` on PATH is it, whatever release the host installs.
    let out = world.command("sh", &worktree).args(["-c", flows]).output().unwrap();
    assert!(out.status.success(), "{flows}: {}", String::from_utf8_lossy(&out.stdout));
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(answer["line"].as_str().unwrap().contains("--repo acme/tracker"), "{answer}");
}

#[test]
fn a_blocker_of_another_checkout_serving_the_tracker_is_worked() {
    let world = world();
    let other = world.second_repo();
    world.name_origin(&other, "acme/kernel");
    world.say_tracker(&other, TRACKER);
    tracker_issue(&world, 7, &[MINE, "ready-for-agent"]);
    world.with_tracker(|gh| {
        gh["edges"]["7"] = json!([
            {"number": 41, "state": "open", "repo": TRACKER, "labels": ["repo:acme/kernel"]}
        ])
    });
    let plan = world.afk(&["plan"]).ok();
    assert_eq!(plan["findings"], json!([]), "{plan}");
    assert_eq!(plan["excluded"][0]["reason"], "open blockers: #41");

    // Outside the tracker, the same repo's issue is unworked. Its known
    // checkout supplies the ready label and repo label for the repair.
    world.with_tracker(|gh| gh["edges"]["7"][0]["repo"] = json!("acme/kernel"));
    let plan = world.afk(&["plan"]).ok();
    let fix = plan["findings"][0]["fix"].as_str().unwrap();
    assert!(fix.contains("--add-label 'repo:acme/kernel' --add-label 'ready-for-agent'"), "{fix}");
    assert!(fix.contains(&other.display().to_string()), "{fix}");
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

#[test]
fn watching_reads_the_trackers_endings_and_keeps_same_numbered_repos_apart() {
    let world = world();
    let second = world.second_repo();
    world.list_checkout(&world.clone);
    world.list_checkout(&second);
    tracker_issue(&world, 7, &[MINE]);
    tracker_issue(&world, 8, &["repo:acme/other"]);
    tracker_issue(&world, 9, &[MINE, "repo:acme/other"]);
    world.with_tracker(|gh| {
        for row in gh["issues"].as_array_mut().unwrap() {
            row["state"] = json!("closed");
            row["closed_at"] = json!(1050);
        }
    });
    world.pr_closing(101, "acme/tracker#7", "work");
    world.with_gh(|gh| {
        gh["prs"][0]["state"] = json!("merged");
        gh["prs"][0]["merged_at"] = json!(1060);
        gh["prs"][0]["updated_at"] = json!(1060);
    });
    // The checkout's own issues never leak into its external tracker's report.
    world.issue(7, &[]);
    world.with_gh(|gh| {
        gh["issues"][0]["state"] = json!("closed");
        gh["issues"][0]["closed_at"] = json!(1070);
    });
    let answer = world.afk(&["doctor", "--all", "--since", "1000", "--now", "1100"]).ok();
    assert_eq!(answer["until"], 1100);
    let rows = answer["checkouts"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let watching = &rows[0]["doctor"]["watching"];
    assert_eq!(watching["issues"].as_array().unwrap().len(), 1, "{watching}");
    assert_eq!(watching["issues"][0]["issue"], 7);
    assert_eq!(watching["ended"][0]["at"], 1050);
    assert_eq!(watching["ended"][1]["step"], "pr_merged");
    assert_eq!(rows[1]["doctor"]["watching"]["issues"], json!([]));
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

/// The repo a call names, and what of it: an endpoint's path after the repo,
/// empty of the repo itself. The tracker is named by name, and the checkout's repo by `{owner}/{repo}`,
/// which `gh` fills in from the checkout: a query names its repo by its
/// `owner` and `repo`, and `gh pr` the checkout's by naming none, or the
/// team's explicitly when it lists open proposals.
fn named(call: &[String]) -> (String, String) {
    let words: Vec<&str> = call.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["pr", verb, ..] => {
            let repo = call
                .windows(2)
                .find(|pair| pair[0] == "--repo")
                .map_or("{owner}/{repo}", |pair| pair[1].as_str());
            return (repo.to_string(), format!("pr {verb}"));
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
    let endpoint = call
        .iter()
        .find_map(|arg| arg.strip_prefix("repos/"))
        .unwrap_or_else(|| panic!("an endpoint: {call:?}"));
    let parts: Vec<&str> = endpoint.splitn(3, '/').collect();
    (format!("{}/{}", parts[0], parts[1]), parts.get(2).unwrap_or(&"").to_string())
}

/// Every `gh` call made of `world`'s checkout and its worktrees → those of
/// them made of the tracker, once it is shown that each is of an issue or a
/// label — or of who may see the tracker, which a start asks — and was
/// answered from the tracker's state; that each made of the
/// checkout's repo is of a PR — its comments among them — or a branch; and
/// that each made of another repo reads a team's or a role's head, or the
/// team's open proposals.
fn routed(world: &World) -> Vec<Vec<String>> {
    let gh = world.gh();
    let prs: Vec<String> =
        gh["prs"].as_array().unwrap().iter().map(|pr| pr["number"].to_string()).collect();
    let on_prs: Vec<&Value> =
        prs.iter().flat_map(|pr| gh["comments"][pr].as_array()).flatten().collect();
    let calls: Vec<Vec<String>> = serde_json::from_value(gh["calls"].clone()).unwrap();
    let mut of_tracker = vec![];
    for call in &calls {
        // Authentication, quota and checkout identity observations are not
        // issue or PR operations to route to the tracker.
        if call.starts_with(&["config".into(), "get".into(), "user".into()])
            || call.starts_with(&["api".into(), "rate_limit".into()])
            || call == &["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]
        {
            continue;
        }
        let (repo, what) = named(call);
        match repo.as_str() {
            TRACKER => {
                let issues = what.starts_with("issues") || what.starts_with("labels");
                let seen =
                    what.is_empty() && call.contains(&afkconvoy::gh::VISIBILITY_JQ.to_string());
                assert!(issues || seen || what == "graphql", "{call:?}");
                of_tracker.push(call.clone());
            }
            "{owner}/{repo}" => {
                let rest = what.strip_prefix("issues/");
                let on = rest.and_then(|rest| rest.split('/').next()).unwrap_or_default();
                let of_pr = what.starts_with("pr ")
                    || what.starts_with("pulls?")
                    || what.starts_with("branches/")
                    || prs.iter().any(|pr| pr == on)
                    || rest.and_then(|rest| rest.strip_prefix("comments/")).is_some_and(|id| {
                        on_prs.iter().any(|comment| comment["id"].as_u64() == id.parse().ok())
                    });
                assert!(of_pr, "a call of the checkout's repo is of a PR or a branch: {call:?}");
            }
            other => {
                assert!(!gh["repos"][other].is_null(), "a call of an unknown repo: {call:?}");
                let proposals = other == "acme/team" && what == "pr list";
                assert!(
                    ["clone", "commits/HEAD"].contains(&what.as_str()) || proposals,
                    "{call:?}"
                );
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
    ] {
        assert!(brief.contains(said), "{said}: {brief}");
    }
    // A commit pushed on the branch the claim named, a PR in the checkout's
    // repo, the gate, and a join.
    commit(&worktree, "seven.txt", "7\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);
    world.pr_closing(70, "acme/tracker#7", &branch);
    let gated = world.afk_in(&worktree, &["gate"]).ok();
    assert_eq!(
        (gated["status"].as_str(), gated["recorded"].as_bool()),
        (Some("green"), Some(true))
    );
    let joined = world.afk_in(&worktree, &["land", "--issue", "7", "--now", "1050"]).ok();
    assert_eq!(joined["outcome"], "merged", "{joined}");

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

const ROLES: &str = "roles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n  lander: acme/lander\n  clerk: acme/clerk\n";
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
/// `build`, then `review`, then a turn that lands its PR.
const SHIP: &str = "\
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
    on: { pass: land }
land: { by: lander, do: join }
```
";
const SWEEP: &str = "```yaml\nby: dev\ndo: sweep\nevery: 1h\nmax_open: 2\n```\n";

/// `world`, its repo bound to a team with the workflows `feature` and `ship`
/// and the patrol `sweep`.
fn bound() -> World {
    bound_with("", "")
}

/// `bound`, the team's `team.md` saying `team` beside its roles, and the
/// repo's config `config` beside naming the team.
fn bound_with(team: &str, config: &str) -> World {
    let world = world();
    for (repo, skills) in [
        ("acme/dev", &["implement", "sweep"][..]),
        ("acme/reviewer", &["review"]),
        ("acme/lander", &["join"]),
        ("acme/clerk", &["propose"]),
    ] {
        let files: Vec<(String, String)> = skills
            .iter()
            .map(|name| {
                let file = format!(".claude/skills/{name}/SKILL.md");
                (file, format!("---\nname: {name}\ndescription: {name}\n---\n"))
            })
            .collect();
        let files: Vec<(&str, &str)> =
            files.iter().map(|(file, text)| (file.as_str(), text.as_str())).collect();
        world.repo(repo, &files);
    }
    let team = format!("```yaml\n{ROLES}{team}```\n");
    world.repo(
        "acme/team",
        &[
            ("team.md", &team),
            ("workflows/feature.md", FEATURE),
            ("workflows/ship.md", SHIP),
            ("patrols/sweep.md", SWEEP),
            ("precedents.md", "# Precedents\n"),
        ],
    );
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: \"true\"\nteam: acme/team\n{config}```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.list_checkout(&world.clone);
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
        let gated = world.afk_in(&worktree, &["gate"]).ok();
        assert_eq!(gated["status"], "green", "{gated}");
        let joined = world.afk_in(&worktree, &["land", "--issue", "7", "--now", "1050"]).ok();
        assert_eq!((&joined["outcome"], &joined["pr"]), (&json!("merged"), &json!(pr)), "{joined}");
    }

    assert_eq!(issue_of(&world.tracker_of(TRACKER), 7)["state"], "closed");
    assert_eq!(issue_of(&world.tracker_of(OTHER), 7)["state"], "closed", "its own turn closed it");
    assert_eq!(did(&tick_of(&world.clone, "alpha", "1090")), ["settle 7 done"]);
    let settled = tick_of(&other, "beta", "1090");
    assert_eq!(did(&settled), ["settle 7 done"], "{settled}");
    assert_eq!(issue_of(&world.tracker_of(OTHER), 7)["state"], "closed");
    assert_eq!(world.remote(&["for-each-ref", "refs/convoy/claim/"]), "");
    assert_eq!(world.gh()["issues"], own, "the repo's own #7 is as it was");
}

/// A `run` on the host `h1`: a second of its sleeps takes 10ms, so its next tick
/// comes 0.9s after its first, not 9s.
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "10"];

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

/// What a tick did that came to anything, but for the `stage:` labels.
fn walked(ticked: &Value) -> Vec<String> {
    did(ticked).into_iter().filter(|step| !step.starts_with("stage_label ")).collect()
}

/// Declares `verdict` for `stage` of the tracker's issue 7, as its worker would.
fn declare(world: &World, stage: &str, verdict: &str) {
    let line = ["verdict", verdict, "--issue", "7", "--stage", stage, "--text", "Done."];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let declared = world.afk(&[&line[..], &commits].concat()).ok();
    assert_eq!(declared["declared"], true, "{declared}");
}

/// The worktree a worker on the tracker's issue 7 was last started in.
fn worktree_of_7(world: &World) -> PathBuf {
    let named = |path: &PathBuf| path.to_str().unwrap().contains("acme-tracker-7-");
    world.worktrees().into_iter().rfind(named).expect("a worktree of acme/tracker#7")
}

#[test]
fn a_bound_repos_tracker_issue_goes_through_its_stages_and_lands_in_a_turn() {
    let world = bound();
    tracker_issue(&world, 7, &["flow:ship", MINE]);
    world.issue(7, &["flow:ship"]);
    let own = world.gh()["issues"].clone();

    // `build`, whose worker reads and writes its issue in the tracker, and
    // opens the PR that closes it by the name the repo gives it.
    assert_eq!(walked(&tick_at_work(&world, 1000)), ["dispatch 7 done"]);
    let dir = worktree_of_7(&world);
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(branch, "tester/acme-tracker-7-tracked-7");
    let told = brief(&dir);
    for said in [
        "**Issue:** acme/tracker#7 — tracked 7",
        "`gh issue view 7 --repo acme/tracker --comments`",
        "`gh issue create --repo acme/tracker --label repo:acme/dev`",
    ] {
        assert!(told.contains(said), "{said}: {told}");
    }
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr_closing(70, "acme/tracker#7", &branch);
    let gated = world.afk_in(&dir, &["gate"]).ok();
    assert_eq!((&gated["status"], &gated["recorded"]), (&json!("green"), &json!(true)));
    declare(&world, "build", "pass");
    assert_eq!(walked(&tick_at_work(&world, 1090)), ["advance 7 done", "dispatch 7 done"]);
    // `review`, which passes to the landing train.
    declare(&world, "review", "pass");
    assert_eq!(walked(&tick_at_work(&world, 1180)), ["advance 7 done"]);
    assert_eq!(issue_of(&world.tracker(), 7)["labels"], json!(["flow:ship", MINE, "stage:land"]));

    // The landing worker gates the PR and lands it in one turn.
    assert_eq!(walked(&tick_at_work(&world, 1270)), ["dispatch 7 done"]);
    assert_eq!(worktree_of_7(&world), dir);
    let told = brief(&dir);
    assert!(told.contains("PR #70 is open for it"), "{told}");
    let gated = world.afk_in(&dir, &["gate"]).ok();
    assert_eq!(gated["status"], "green", "{gated}");
    let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1300"]).ok();
    assert_eq!(joined["outcome"], "merged", "{joined}");
    assert_eq!(world.remote(&["show", "refs/heads/main:seven.txt"]), "7");
    let joined = world.remote(&["log", "--format=%s%n%b", "--merges", "-1", "main"]);
    assert_eq!(joined, "pr 70 (#70)\nCloses acme/tracker#7");
    assert_eq!(issue_of(&world.tracker(), 7)["state"], "closed");
    // The landing worker woke the fleet and stopped at its prompt.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": 1400 * 1000});
        }
    });
    assert_eq!(walked(&tick(&world, "1450")), ["settle 7 done"]);
    assert_eq!(world.claims(), []);

    // Each stage's record is on the tracker's issue; the repo's are as they were.
    let said = comments_of(&world.tracker(), 7).join("\n");
    for record in ["stage=build verdict=pass", "stage=review verdict=pass"] {
        assert!(said.contains(record), "{record}: {said}");
    }
    assert_eq!(world.gh()["issues"], own);
    routed(&world);
}

/// A worker of `dev` takes the tracker's issue `issue` in to `feature`, on
/// what `on` says: `--patrol` or `--from`, and what it waits for.
fn take_in(world: &World, issue: u64, on: &[&str]) -> common::Answer {
    let issue = issue.to_string();
    let line = ["intake", "--issue", &issue, "--workflow", "feature", "--role", "dev"];
    world.afk(&[&line[..], on].concat())
}

#[test]
fn a_patrols_and_a_splits_issues_are_the_trackers_with_the_repo_label_and_the_intake_limit_counts_the_repos()
 {
    let world = bound_with("", "intake_limit: 3\npatrols: [sweep]\n");
    // `flows` makes the flow labels in the tracker, and gives the line that
    // opens an issue there.
    let flows = world.afk(&["flows"]).ok();
    assert_eq!(flows["created"], json!(["flow:feature", "flow:ship"]), "{flows}");
    let labels = world.tracker()["labels"].clone();
    assert!(labels.as_array().unwrap().contains(&json!("flow:ship")), "{labels}");
    assert_eq!(
        flows["workflows"][0]["line"],
        "gh issue create --repo acme/tracker --label 'flow:feature' --label 'repo:acme/dev' \
         --title '<title>' --body-file <file>"
    );
    assert_eq!(flows["intake"], json!({"open": 0, "limit": 3}), "{flows}");

    // Another repo's issue, taken in by its own fleet's patrol: not this
    // fleet's to count.
    tracker_issue(&world, 20, &["flow:feature", "repo:acme/web"]);
    world.with_tracker(|gh| {
        let record = "<!--convoy:intake workflow=feature role=dev patrol=sweep-->\ntaken in";
        let comment = json!({"id": 2001, "body": record, "html_url": "https://gh/c/2001"});
        gh["comments"]["20"] = json!([comment]);
    });

    // The patrol's worker is told to open its issues in the tracker, with the
    // repo label, and to look there for one that covers what it found.
    assert_eq!(did(&tick(&world, "10000")), ["patrol done"]);
    let checkout = world
        .worktrees()
        .into_iter()
        .find(|path| path.to_str().unwrap().contains("/convoy-patrol-sweep"));
    let told = brief(&checkout.expect("the patrol's checkout"));
    for said in [
        "`gh issue create --repo acme/tracker --label repo:acme/dev`",
        "`gh issue list --repo acme/tracker --label repo:acme/dev --state open`",
    ] {
        assert!(told.contains(said), "{said}: {told}");
    }
    // It opens one with the repo label and one without: intake gives it.
    tracker_issue(&world, 12, &[MINE]);
    tracker_issue(&world, 13, &[]);
    assert_eq!(take_in(&world, 12, &["--patrol", "sweep"]).ok()["open"], 1);
    assert_eq!(take_in(&world, 13, &["--patrol", "sweep"]).ok()["open"], 2);
    let tracker = world.tracker();
    for number in [12, 13] {
        assert_eq!(issue_of(&tracker, number)["labels"], json!([MINE, "flow:feature"]));
        let record = &comments_of(&tracker, number)[0];
        assert!(record.starts_with("<!--convoy:intake workflow=feature role=dev patrol=sweep-->"));
    }
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "10001"]).ok();
    assert_eq!(plan["patrols"][0]["open"], 2, "{plan}");

    // A stage's worker splits its issue: the child, and the edge it waits
    // on, are the tracker's.
    tracker_issue(&world, 15, &[]);
    tracker_issue(&world, 16, &[MINE]);
    let split = take_in(&world, 15, &["--from", "7", "--blocked-by", "16"]).ok();
    assert_eq!((&split["open"], &split["limit"]), (&json!(3), &json!(3)), "{split}");
    assert_eq!(world.tracker()["edges"]["15"], json!([16]));
    assert_eq!(issue_of(&world.tracker(), 15)["labels"], json!([MINE, "flow:feature"]));

    // Three of the repo's are open, whatever the other repo's: no more.
    tracker_issue(&world, 17, &[MINE]);
    let full = take_in(&world, 17, &["--from", "7"]).error();
    assert!(full.contains("the intake limit is reached: 3 open issues"), "{full}");
    // Another repo's issue is that repo's to take in.
    tracker_issue(&world, 18, &["repo:acme/web"]);
    let theirs = take_in(&world, 18, &["--from", "7"]).error();
    assert!(
        theirs.contains("issue acme/tracker#18 carries the repo label of another repo"),
        "{theirs}"
    );
    assert_eq!(issue_of(&world.tracker(), 18)["labels"], json!(["repo:acme/web"]));
    assert_eq!(comments_of(&world.tracker(), 18), Vec::<String>::new());

    // `flows` counts the same.
    assert_eq!(world.afk(&["flows"]).ok()["intake"], json!({"open": 3, "limit": 3}));
    assert_eq!(world.gh()["issues"], json!([]), "the repo has no issue of its own");
    routed(&world);
}

#[test]
fn a_lock_raised_on_a_tracker_issue_is_given_the_repo_label_and_one_of_another_repo_is_refused() {
    let world = bound();
    tracker_issue(&world, 7, &["flow:feature", MINE]);
    tracker_issue(&world, 9, &["bug"]);
    tracker_issue(&world, 10, &["bug", "repo:acme/web"]);

    let raised = world.afk(&["lock", "--issue", "9", "--role", "dev"]).ok();
    assert_eq!((&raised["raised"], &raised["locks"]), (&json!(true), &json!([9])), "{raised}");
    let tracker = world.tracker();
    assert_eq!(issue_of(&tracker, 9)["labels"], json!(["bug", MINE, "fleet-lock"]));
    assert!(comments_of(&tracker, 9)[0].starts_with("<!--convoy:lock"), "{tracker}");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!((&plan["locks"], &plan["frontier"]), (&json!([9]), &json!([])), "{plan}");

    // The lock of another repo's issue would hold that repo's fleet, and not
    // this one: it is refused, and nothing is written.
    let refused = world.afk(&["lock", "--issue", "10", "--role", "dev"]).error();
    assert!(
        refused.contains("issue acme/tracker#10 carries the repo label of another repo"),
        "{refused}"
    );
    let tracker = world.tracker();
    assert_eq!(issue_of(&tracker, 10)["labels"], json!(["bug", "repo:acme/web"]));
    assert_eq!(comments_of(&tracker, 10), Vec::<String>::new());
    assert_eq!(world.gh()["issues"], json!([]));
    routed(&world);
}

#[test]
fn rule_and_send_steer_the_trackers_issue_without_proposing_a_precedent() {
    let world = bound_with("", "");
    tracker_issue(&world, 7, &["flow:feature", MINE]);
    world.issue(7, &["flow:feature"]);
    let own = world.gh()["issues"].clone();
    assert_eq!(walked(&tick_at_work(&world, 1000)), ["dispatch 7 done"]);
    declare(&world, "build", "escalate");
    assert_eq!(walked(&tick_at_work(&world, 1090)), ["advance 7 done"]);
    let labels = issue_of(&world.tracker(), 7)["labels"].clone();
    assert!(labels.as_array().unwrap().contains(&json!("ready-for-human")), "{labels}");
    let said = comments_of(&world.tracker(), 7).join("\n");
    assert!(said.contains("gh issue close 7 --repo acme/tracker"), "{said}");

    // A human rules: the tracker's issue goes back to its stage.
    let ruled = world.afk(&["rule", "--issue", "7", "--text", "Keep the v1 schema."]).ok();
    assert_eq!((&ruled["ruled"], &ruled["relabelled"]), (&json!(true), &json!(true)), "{ruled}");
    let labels = issue_of(&world.tracker(), 7)["labels"].clone();
    assert!(labels.as_array().unwrap().contains(&json!("flow:feature")), "{labels}");
    assert!(!labels.as_array().unwrap().contains(&json!("ready-for-human")), "{labels}");
    let tracker = world.tracker();
    let comments = tracker["comments"]["7"].as_array().unwrap();
    let ruling = comments
        .iter()
        .rfind(|comment| comment["body"].as_str().unwrap().contains(" verdict=ruling by=human-->"))
        .expect("the ruling, on the tracker's issue");
    assert!(ruling["body"].as_str().unwrap().contains("Keep the v1 schema."));
    // The stage runs again with its ruling and no additional worker.
    assert_eq!(walked(&tick_at_work(&world, 1180)), ["dispatch 7 done"]);
    assert!(!comments_of(&world.tracker(), 7).join("\n").contains("convoy:precedent-proposed"));
    assert!(!world.worktrees().iter().any(|path| {
        path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("convoy-precedent-"))
    }));

    // A human sends it on to `review`, once its worker has let it go.
    world.afk(&["release", "--issue", "7", "--instance", "alpha", "--host", "h1"]).ok();
    let line = ["send", "--issue", "7", "--stage", "review", "--text", "Review it."];
    let sent = world.afk(&line).ok();
    assert_eq!((&sent["stage"], &sent["sent"]), (&json!("review"), &json!(true)), "{sent}");
    let said = comments_of(&world.tracker(), 7).join("\n");
    assert!(said.contains("a human sent this issue to the stage `review`"), "{said}");
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    assert_eq!(plan["frontier"][0]["stage"], "review", "{plan}");

    assert_eq!(world.gh()["issues"], own);
    assert_eq!(world.gh()["comments"], json!({}), "nothing is said on the checkout's repo");
}

#[test]
fn a_model_label_on_the_trackers_issue_is_followed_and_cast_adds_none() {
    let world = world();
    world.hold_every_harness();
    world.kit_models("pi", &["a/fast", "a/slow"]);
    world.afk(&["switch", "--harness", "pi"]).ok();
    assert!(world.afk(&["kit", "--cast", "on"]).error().contains("automatic casting is gone"));
    assert!(world.afk(&["cast", "--now", "1000"]).error().contains("automatic casting is gone"));
    tracker_issue(&world, 7, &["ready-for-agent", MINE, "model:a/fast"]);
    world.issue(7, &["ready-for-agent"]);
    let own = world.gh()["issues"].clone();
    let planned = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "pi");
    assert_eq!(planned["frontier"][0]["cast"]["model"], "a/fast");
    assert_eq!(planned["frontier"][0]["cast"]["why"], "model_label");
    let tracker = world.tracker();
    assert_eq!(issue_of(&tracker, 7)["labels"], json!(["ready-for-agent", MINE, "model:a/fast"]));
    assert_eq!(world.gh()["issues"], own, "nothing is said on the checkout's repo");
    assert_eq!(world.gh()["comments"], json!({}));
    routed(&world);
}

/// The team's roster names a model for `reviewer` that the host kit holds by
/// no name.
const UNMATCHED: &str = "roster:\n  reviewer: { run: [{ harness: claude-code, model: opuss }] }\n";

#[test]
fn a_roster_issue_of_a_bound_repo_is_opened_in_the_teams_repo_and_not_the_tracker() {
    let world = bound_with(UNMATCHED, "");
    world.kit_models("claude-code", &["opus"]);
    tracker_issue(&world, 7, &["flow:feature", MINE]);
    tick_at_work(&world, 1000);
    let opened = world.gh()["other_issues"]["acme/team"].clone();
    assert_eq!(opened.as_array().map(Vec::len), Some(1), "{opened}");
    assert!(opened[0]["body"].as_str().unwrap().contains("opuss"), "{opened}");
    let tracker = world.tracker();
    let numbers: Vec<&Value> =
        tracker["issues"].as_array().unwrap().iter().map(|i| &i["number"]).collect();
    assert_eq!(numbers, [&json!(7)], "nothing is opened in the tracker");
}

/// sunfmin/control#553: a fleet whose checkout the list holds no entry of
/// yet — as through its first tick, and the whole of a `run --foreground` —
/// gives the worker it dispatches its tracker, so the worker's commands act
/// on the tracker's issues and not on its repo's own of the same number.
#[test]
fn a_workers_command_goes_by_its_fleets_tracker_before_the_list_holds_its_checkout() {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, "acme/dev");
    world.put_tracker(TRACKER);
    tracker_issue(&world, 7, &["ready-for-agent", MINE]);
    // The checkout's repo has an issue of the same number, which no worker's
    // command may take for the fleet's.
    world.issue(7, &["ready-for-agent"]);
    let own = world.gh()["issues"].clone();

    let run = [
        "run",
        "--tracker",
        TRACKER,
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        "1000",
        "--ticks",
        "1",
        "--second-ms",
        "0",
    ];
    world.afk_lines(&run);
    assert_eq!(world.checkout_list()["checkouts"], json!([]), "the list holds no entry");
    let worktree = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    let branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(branch, "tester/acme-tracker-7-tracked-7");

    // The worker's commands: the tracker's issue is the one landed and closed.
    commit(&worktree, "seven.txt", "7\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);
    world.pr_closing(70, "acme/tracker#7", &branch);
    let joined = world.afk_in(&worktree, &["land", "--issue", "7", "--now", "1050"]).ok();
    assert_eq!(joined["outcome"], "merged", "{joined}");
    assert_eq!(issue_of(&world.tracker(), 7)["state"], "closed");
    assert_eq!(world.gh()["issues"], own, "the repo's own #7 is as it was");

    // A worker's worktree whose tracker nothing says: its command is refused,
    // never read as the repo's own.
    let git_dir = PathBuf::from(git(&worktree, &["rev-parse", "--absolute-git-dir"]));
    std::fs::remove_file(git_dir.join("convoy-worker-tracker")).unwrap();
    let refused = world.afk_in(&worktree, &["land", "--issue", "7", "--now", "1060"]);
    assert_ne!(refused.code, 0, "{}", refused.json);
    let error = refused.json["error"].as_str().unwrap_or_default();
    assert!(error.contains("cannot be read"), "{}", refused.json);
}

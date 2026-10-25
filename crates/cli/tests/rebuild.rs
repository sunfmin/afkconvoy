//! What a rebuild costs: GitHub is asked a fixed number of times and the
//! remote is listed once, however many issues are in flight.

use crate::common;

use common::{World, git, write};
use serde_json::Value;

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

/// A world whose repo is bound to a team with the one workflow `feature`,
/// and whose startup probe passed at 1000.
fn bound() -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    world.repo(
        "acme/reviewer",
        &[(".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n")],
    );
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    plan(&world);
    world
}

/// A plan — a tick stopped after its rebuild — while the probe stands.
fn plan(world: &World) -> Value {
    world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok()
}

/// The stage each issue on the frontier is at.
fn frontier(plan: &Value) -> Vec<(u64, &str)> {
    let rows = plan["frontier"].as_array().unwrap().iter();
    rows.map(|row| (row["number"].as_u64().unwrap(), row["stage"].as_str().unwrap())).collect()
}

/// The `gh` calls the rebuild of a repo with `flowing` issues that follow a
/// workflow makes, each as its first two words — sorted: they are made side
/// by side.
fn asked_of_github(flowing: u64) -> Vec<String> {
    let world = bound();
    for number in 1..=flowing {
        world.issue(number, &["flow:feature"]);
        world.comment(number, "a word on it");
    }
    // Every flowing issue has a dependency, whose own state is closed even
    // while GitHub's summary claims it is open. No per-issue read is needed.
    world.issue(999, &["bug"]);
    world.close(999, "completed");
    world.with_gh(|gh| {
        for number in 1..=flowing {
            gh["edges"][number.to_string()] = serde_json::json!([999]);
        }
        for issue in gh["issues"].as_array_mut().unwrap() {
            issue["blocked_by"] = serde_json::json!(1);
        }
    });
    // the team is kept on this host by the first read of it
    plan(&world);
    world.gh_calls();
    let planned = plan(&world);
    assert_eq!(frontier(&planned).len() as u64, flowing, "{planned}");
    let mut asked: Vec<String> = world.gh_calls().iter().map(|call| common::asked(call)).collect();
    asked.sort();
    asked
}

#[test]
fn a_rebuild_asks_github_as_often_of_forty_issues_in_flight_as_of_four() {
    let (four, forty) = (asked_of_github(4), asked_of_github(40));
    assert_eq!(four, forty);
    assert!(four.contains(&"api graphql".to_string()), "{four:?}");
    assert!(four.contains(&"pr list".to_string()), "{four:?}");
    assert_eq!(four.iter().filter(|asked| *asked == "api graphql").count(), 1, "{four:?}");
}

#[test]
fn a_rebuild_lists_the_remote_once() {
    let world = bound();
    for number in [7, 8] {
        world.issue(number, &["flow:feature"]);
    }
    for (issue, instance) in [("7", "alpha"), ("8", "beta")] {
        world.afk(&["claim", "--issue", issue, "--instance", instance, "--now", "1000"]).ok();
    }
    // a train one PR has joined: the line is read off the same listing, and
    // so are the version tags a cut counts from
    let head = git(&world.clone, &["rev-parse", "HEAD"]);
    git(&world.clone, &["push", "-q", "origin", &format!("{head}:refs/convoy/train/line/main")]);

    world.watch_git();
    let planned = plan(&world);
    assert_eq!(planned["mine"][0]["number"], 7, "{planned}");
    assert_eq!(planned["peer_live"][0]["number"], 8, "{planned}");
    let calls = world.git_calls();
    let listed: Vec<&String> = calls.iter().filter(|call| call.starts_with("ls-remote")).collect();
    let once = "ls-remote origin refs/convoy/* refs/heads/main refs/tags/v*";
    assert_eq!(listed, [once], "{calls:#?}");
}

#[test]
fn an_issue_with_more_comments_than_come_with_it_is_read_to_its_last() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    for word in 0..120 {
        world.comment(7, &format!("word {word}"));
    }
    // its stage record is past the first hundred
    world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "1000"]).ok();
    let verdict = ["verdict", "pass", "--issue", "7", "--stage", "build", "--text", "built"];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    world.afk(&[&verdict[..], &commits].concat()).ok();
    world.afk(&["release", "--issue", "7", "--instance", "alpha"]).ok();

    world.gh_calls();
    assert_eq!(frontier(&plan(&world)), [(7, "review")]);
    let again: Vec<Vec<String>> = world
        .gh_calls()
        .into_iter()
        .filter(|call| call.iter().any(|arg| arg.contains("/issues/7/comments")))
        .collect();
    assert_eq!(again.len(), 1, "it alone is asked about again: {again:?}");
}

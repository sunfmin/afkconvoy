//! `check`: a team and its workflows are read — the team's files and each
//! role's repo — and either accepted, with what was read, or refused with
//! every fault and the file and key it is in.

mod common;

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str =
    "# The team\n\n```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
# Feature

```yaml
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: reviewer, do: rule }
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: land, reject: build }
land: { by: dev, do: land }
```
";

fn skill(name: &str) -> (String, &'static str) {
    (format!(".claude/skills/{name}/SKILL.md"), "a skill\n")
}

/// A world whose GitHub holds two roles and a team of `files`, with the
/// repo's config naming the team.
fn bound(files: &[(&str, &str)]) -> World {
    let world = World::new();
    for (role, skills) in
        [("acme/dev", &["implement", "land"][..]), ("acme/reviewer", &["review", "rule"])]
    {
        let skills: Vec<_> = skills.iter().map(|name| skill(name)).collect();
        let files: Vec<(&str, &str)> =
            skills.iter().map(|(file, text)| (file.as_str(), *text)).collect();
        world.repo(role, &files);
    }
    world.repo("acme/team", files);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    world
}

fn head(world: &World, repo: &str) -> String {
    git(&world.root.join("github").join(repo), &["rev-parse", "HEAD"])
}

/// The faults of a refused team, each as "file: key".
fn faults(refused: &Value) -> Vec<String> {
    let faults = refused["faults"].as_array().unwrap().iter();
    faults
        .map(|f| format!("{}: {}", f["file"].as_str().unwrap(), f["key"].as_str().unwrap()))
        .collect()
}

#[test]
fn a_well_formed_team_is_accepted_with_what_was_read() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["team"], "acme/team");
    assert_eq!(read["commit"], head(&world, "acme/team"));
    assert_eq!(read["files"], json!(["team.md", "workflows/feature.md"]));
    assert_eq!(read["escalate_label"], "ready-for-human");
    assert_eq!(
        read["roles"],
        json!([
            {"name": "dev", "repo": "acme/dev", "commit": head(&world, "acme/dev"),
             "skills": ["implement", "land"]},
            {"name": "reviewer", "repo": "acme/reviewer", "commit": head(&world, "acme/reviewer"),
             "skills": ["review", "rule"]},
        ])
    );
    assert_eq!(
        read["workflows"],
        json!([{
            "name": "feature",
            "flow_label": "flow:feature",
            "max_rework": 3,
            "max_consecutive_rework": 2,
            "stages": [
                {"name": "build", "by": "dev", "do": "implement", "model": null, "writes": true,
                 "escalate": {"to": "reviewer", "do": "rule"}, "pass": "review", "reject": null},
                {"name": "review", "by": "reviewer", "do": "review", "model": null,
                 "writes": false, "escalate": null, "pass": "land", "reject": "build"},
            ],
            "land": {"by": "dev", "do": "land"},
        }])
    );
    // GitHub was only read: the team's repo and each role's, once.
    let calls = world.gh_calls();
    assert!(calls.iter().all(|call| call[..2] == ["repo", "clone"]), "{calls:?}");
    assert_eq!(calls.len(), 3);
}

#[test]
fn every_fault_of_a_team_is_reported_in_one_run_with_its_file_and_key() {
    let team = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n  ops: acme/ops\nowner: me\n```\n";
    let feature = FEATURE
        .replace("do: implement", "do: deploy")
        .replace("reject: build", "reject: review")
        .replace("land: { by: dev, do: land }", "land: { by: qa, do: land }\nmax_rework: many");
    let small = "```yaml\nstages:\n  - { name: done, by: dev, do: implement, on: { pass: nowhere } }\n```\n";
    let world = bound(&[
        ("team.md", team),
        ("workflows/feature.md", &feature),
        ("workflows/small.md", small),
        ("workflows/notes.txt", "not a workflow\n"),
        ("workflows/empty.md", "no block here\n"),
    ]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(
        faults(&answer.json),
        [
            "team.md: owner",
            "team.md: roles.ops",
            "workflows/empty.md: ",
            "workflows/feature.md: max_rework",
            "workflows/feature.md: land.by",
            "workflows/feature.md: stages[0].do",
            "workflows/feature.md: stages[1].on.reject",
            "workflows/notes.txt: ",
            "workflows/small.md: stages[0].name",
            "workflows/small.md: stages[0].on.pass",
        ]
    );
    let error = answer.error();
    assert!(error.starts_with("the team acme/team has 10 faults"), "{error}");
    for said in [
        "team.md: owner: unknown key",
        "team.md: roles.ops: acme/ops cannot be read: gh repo clone failed",
        "workflows/empty.md: has no ```yaml block",
        "workflows/feature.md: stages[0].do: `deploy` is not a skill of the role `dev`",
        "workflows/feature.md: stages[1].on.reject: `review` is its own stage",
    ] {
        assert!(error.contains(said), "{said:?} is not in {error}");
    }
}

#[test]
fn a_role_is_read_at_the_head_of_its_default_branch() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let dev = world.root.join("github/acme/dev");
    let before = world.afk(&["check"]).ok();
    // A skill on another branch is not the role's; one pushed to the default
    // branch is, from the next read on.
    git(&dev, &["checkout", "-q", "-b", "draft"]);
    commit(&dev, ".claude/skills/draft/SKILL.md", "a draft\n");
    git(&dev, &["checkout", "-q", "main"]);
    assert_eq!(world.afk(&["check"]).ok()["roles"], before["roles"]);
    let pushed = commit(&dev, ".claude/skills/plan/SKILL.md", "a skill\n");
    let after = world.afk(&["check"]).ok();
    assert_ne!(before["roles"][0]["commit"], after["roles"][0]["commit"]);
    assert_eq!(after["roles"][0]["commit"], pushed);
    assert_eq!(after["roles"][0]["skills"], json!(["implement", "land", "plan"]));
}

#[test]
fn a_team_is_checked_from_a_directory_as_it_is_or_by_name() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    // An edit in a checkout of the team, not committed.
    let team = world.root.join("github/acme/team");
    write(&team, "workflows/feature.md", &FEATURE.replace("pass: review", "pass: reveiw"));
    let dir = team.to_str().unwrap();
    let answer = world.afk_in(&world.root, &["check", "--dir", dir]);
    assert_eq!(faults(&answer.json), ["workflows/feature.md: stages[0].on.pass"]);
    assert!(answer.error().contains("has 1 fault\n"));
    write(&team, "workflows/feature.md", FEATURE);
    let read = world.afk_in(&world.root, &["check", "--dir", dir]).ok();
    assert_eq!((&read["team"], &read["commit"]), (&json!(dir), &Value::Null));
    assert_eq!(read["roles"][0]["commit"], head(&world, "acme/dev"), "roles come from GitHub");
    // By name, from anywhere: what is committed.
    let read = world.afk_in(&world.root, &["check", "--team", "acme/team"]).ok();
    assert_eq!(read["commit"], head(&world, "acme/team"));
    assert!(world.afk(&["check", "--team", "acme"]).error().contains("owner/repo"));
    assert!(world.afk(&["check", "--dir", "/nowhere"]).error().contains("not a directory"));
}

#[test]
fn a_team_that_cannot_be_read_and_an_unbound_repo_are_errors() {
    let world = World::new();
    assert!(world.afk(&["check"]).error().contains("this repo is unbound"));
    let error = world.afk(&["check", "--team", "acme/gone"]).error();
    assert!(error.contains("the team acme/gone cannot be read"), "{error}");
    // A repo with nothing in it is a team with no team file.
    world.repo("acme/blank", &[]);
    let answer = world.afk(&["check", "--team", "acme/blank"]);
    assert_eq!(faults(&answer.json), ["team.md: "]);
    // The least a team is: a team file. It has no workflow yet.
    world.repo("acme/least", &[("team.md", "```yaml\n{}\n```\n")]);
    let read = world.afk(&["check", "--team", "acme/least"]).ok();
    assert_eq!((&read["roles"], &read["workflows"]), (&json!([]), &json!([])));
}

#[test]
fn a_bound_repos_config_answers_its_four_keys_more() {
    let world = World::new();
    let config = |yaml: &str| {
        let text = format!("```yaml\nbase_branch: main\ngate_command: make\n{yaml}\n```\n");
        write(&world.clone, "docs/agents/afkconvoy.md", &text);
        world.afk(&["config"])
    };
    let bound = config("team: acme/team").ok();
    assert_eq!(bound["team"], "acme/team");
    assert_eq!(bound["patrols"], json!([]));
    assert_eq!(bound["lock_label"], "fleet-lock");
    assert_eq!(bound["intake_limit"], 50);
    assert_eq!(bound["concurrency"], 3, "the keys every repo has are read as before");
    let set = config("team: acme/team\npatrols: [triage]\nlock_label: stop\nintake_limit: 9").ok();
    assert_eq!(set["patrols"], json!(["triage"]));
    assert_eq!((&set["lock_label"], &set["intake_limit"]), (&json!("stop"), &json!(9)));
    for (yaml, key) in [
        ("team: acme", "team"),
        ("team: [acme/team]", "team"),
        ("team: acme/team\nintake_limit: -1", "intake_limit"),
        ("team: acme/team\npatrols: triage", "patrols"),
        ("patrols: [triage]", "patrols"),
        ("lock_label: stop", "lock_label"),
        ("intake_limit: 5", "intake_limit"),
    ] {
        let error = config(yaml).error();
        assert!(error.contains(key), "{yaml:?} → {error}");
    }
}

/// The help is where an agent learns a team's files: it names every key the
/// loader reads.
#[test]
fn the_help_names_every_key_of_a_definition() {
    use afkconvoy_core::team::{
        ESCALATE_KEYS, LAND_KEYS, ON_KEYS, STAGE_KEYS, TEAM_KEYS, WORKFLOW_KEYS,
    };
    let world = World::new();
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root)
        .args(["check", "--help"])
        .output();
    let help = String::from_utf8(out.unwrap().stdout).unwrap();
    let keys = [&TEAM_KEYS[..], &WORKFLOW_KEYS, &STAGE_KEYS, &ON_KEYS, &ESCALATE_KEYS, &LAND_KEYS];
    for key in keys.concat() {
        assert!(help.contains(&format!(" {key}:")), "`check --help` never shows the key {key}");
    }
}

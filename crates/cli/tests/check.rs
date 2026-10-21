//! `check`: a team and its workflows are read — the team's files and each
//! role's repo — and either accepted, with what was read, or refused with
//! every fault and the file and key it is in.

use crate::common;

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

/// The file of the skill `name`, in the form `check` holds one to.
fn skill_file(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: A skill. Use when the brief names it.\n---\n\n# {name}\n"
    )
}

fn skill(name: &str) -> (String, String) {
    (format!(".claude/skills/{name}/SKILL.md"), skill_file(name))
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
            skills.iter().map(|(file, text)| (file.as_str(), text.as_str())).collect();
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

/// The `see` of a fault: the command that prints a topic, which must print one.
fn see(fault: &Value) -> &str {
    let see = fault["see"].as_str().unwrap_or_else(|| panic!("no `see`: {fault}"));
    let topic = see.strip_prefix("afkconvoy help ").unwrap_or_else(|| panic!("{see}"));
    let mut help = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy"));
    let printed = help.args(["help", topic]).output().unwrap();
    assert!(printed.status.success(), "`{see}` prints nothing");
    see
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
    assert_eq!((&read["team"], &read["path"]), (&json!("acme/team"), &json!("")));
    assert_eq!(read["commit"], head(&world, "acme/team"));
    assert_eq!(read["files"], json!(["team.md", "workflows/feature.md"]));
    assert_eq!(read["escalate_label"], "ready-for-human");
    assert_eq!(read["rules"], false, "the team holds no CLAUDE.md");
    assert_eq!(read["host_user"], true, "its workers are given the host user's skills and rules");
    assert_eq!(
        read["roles"],
        json!([
            {"name": "dev", "repo": "acme/dev", "path": "", "commit": head(&world, "acme/dev"),
             "host_user": true, "skills": ["implement", "land"], "above": [], "limits": []},
            {"name": "reviewer", "repo": "acme/reviewer", "path": "",
             "commit": head(&world, "acme/reviewer"), "host_user": true, "skills": ["review", "rule"], "above": [],
             "limits": []},
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
                {"name": "build", "by": "dev", "do": "implement", "harness": "claude-code",
                 "model": null, "writes": true,
                 "escalate": {"to": "reviewer", "do": "rule"}, "pass": "review", "reject": null},
                {"name": "review", "by": "reviewer", "do": "review", "harness": "claude-code",
                 "model": null, "writes": false, "escalate": {"to": "stock-consult"}, "pass": "land", "reject": "build"},
            ],
            "land": {"by": "dev", "do": "land"},
        }])
    );
    // GitHub was only read: the team's repo and each role's, asked for its
    // head and cloned once.
    assert_eq!(clones(&world), ["acme/dev", "acme/reviewer", "acme/team"]);
}

/// The repos cloned since the last look, sorted — every other `gh` call
/// since being a read through its API.
fn clones(world: &World) -> Vec<String> {
    let calls = world.gh_calls();
    let (clones, reads): (Vec<_>, Vec<_>) =
        calls.iter().partition(|call| call[..2] == ["repo", "clone"]);
    assert!(reads.iter().all(|call| call[0] == "api" && !call.contains(&"--method".into())));
    let mut cloned: Vec<String> = clones.iter().map(|call| call[2].clone()).collect();
    cloned.sort();
    cloned
}

/// A repo read once is not cloned again while its head stays where it was: a
/// command reads it from the fleet's copy of that commit, the one a tick
/// reads, and a push is noticed because the head is asked for every time.
#[test]
fn a_repo_is_cloned_only_when_the_fleet_has_no_copy_of_its_head() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let first = world.afk(&["check"]).ok();
    assert_eq!(clones(&world).len(), 3);
    let copy = |repo: &str| world.cache().join("afkconvoy/copies").join(repo);
    let copied = copy("acme/dev").join(head(&world, "acme/dev"));
    assert!(copied.join(".claude/skills/implement/SKILL.md").is_file());
    // Every command that reads a team reads the copies, and clones nothing.
    assert_eq!(world.afk(&["check"]).ok(), first);
    assert_eq!(world.afk(&["check", "--team", "acme/team"]).ok()["roles"], first["roles"]);
    let dir = world.root.join("github/acme/team");
    let read = world.afk(&["check", "--dir", dir.to_str().unwrap()]).ok();
    assert_eq!(read["roles"], first["roles"]);
    world.issue(7, &["flow:feature"]);
    assert_eq!(world.afk(&["show", "--issue", "7"]).ok()["position"]["stage"], "build");
    world.afk(&["rehearse", "feature", "pass", "pass"]).ok();
    assert_eq!(clones(&world), [] as [&str; 0]);
    // A role pushed since is read at its new head: that repo alone is cloned.
    let pushed = commit(
        &world.root.join("github/acme/dev"),
        ".claude/skills/plan/SKILL.md",
        "---\nname: plan\ndescription: a\n---\n",
    );
    let after = world.afk(&["check"]).ok();
    assert_eq!(after["roles"][0]["commit"], pushed);
    assert_eq!(after["roles"][0]["skills"], json!(["implement", "land", "plan"]));
    assert_eq!(clones(&world), ["acme/dev"]);
    assert!(copy("acme/dev").join(&pushed).is_dir() && copied.is_dir());
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
    // Each names the topic that describes the file it is in.
    let sees: Vec<&str> = answer.json["faults"].as_array().unwrap().iter().map(see).collect();
    let (team, workflows) = ("afkconvoy help team", "afkconvoy help workflows");
    assert_eq!(sees[..2], [team, team]);
    assert!(sees[2..].iter().all(|see| *see == workflows), "{sees:?}");
    let error = answer.error();
    assert!(error.starts_with("the team acme/team has 10 faults"), "{error}");
    for said in [
        "team.md: owner: unknown key",
        "team.md: roles.ops: acme/ops cannot be read: gh api repos/acme/ops/commits/HEAD failed",
        "workflows/empty.md: has no ```yaml block",
        "workflows/feature.md: stages[0].do: `deploy` is not a skill of the role `dev`",
        "workflows/feature.md: stages[1].on.reject: `review` is its own stage",
    ] {
        assert!(error.contains(said), "{said:?} is not in {error}");
    }
}

/// A skill a `do` names is held to the form of a skill's file: one whose
/// `SKILL.md` is missing or malformed is a fault at each `do` that names it,
/// and one no `do` names is not read.
#[test]
fn a_skill_whose_file_is_not_in_the_form_of_one_is_a_fault_at_the_do_that_names_it() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let (dev, reviewer) =
        (world.root.join("github/acme/dev"), world.root.join("github/acme/reviewer"));
    // Nobody is asked to perform `draft`.
    commit(&dev, ".claude/skills/draft/SKILL.md", "a draft\n");
    world.afk(&["check"]).ok();

    commit(&dev, ".claude/skills/implement/SKILL.md", "# implement\n\nBuild it.\n");
    commit(&dev, ".claude/skills/land/SKILL.md", &skill_file("join"));
    commit(&reviewer, ".claude/skills/rule/SKILL.md", "---\nname: rule\n---\n\n# rule\n");
    git(&reviewer, &["rm", "-q", ".claude/skills/review/SKILL.md"]);
    commit(&reviewer, ".claude/skills/review/notes.md", "what a review looks for\n");
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(
        faults(&answer.json),
        [
            "workflows/feature.md: stages[0].do",
            "workflows/feature.md: stages[0].escalate.do",
            "workflows/feature.md: stages[1].do",
            "workflows/feature.md: land.do",
        ]
    );
    let problems: Vec<&str> = answer.json["faults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["problem"].as_str().unwrap())
        .collect();
    for (problem, why) in problems.iter().zip([
        "acme/dev/.claude/skills/implement/SKILL.md does not open with frontmatter: ",
        "acme/reviewer/.claude/skills/rule/SKILL.md says no `description`. ",
        "acme/reviewer/.claude/skills/review/SKILL.md is not there, or is no file. ",
        "acme/dev/.claude/skills/land/SKILL.md says its `name` is `join`, which its directory's is not. ",
    ]) {
        assert!(problem.contains(why), "{problem}");
        assert!(problem.ends_with("`afkconvoy help skill-file` says its form"), "{problem}");
    }
    assert!(problems[0].starts_with("`implement`, the skill the stage `build` of the workflow `feature` has the role `dev` perform, "));

    // Each put right, the team loads.
    for (repo, name) in
        [(&dev, "implement"), (&dev, "land"), (&reviewer, "rule"), (&reviewer, "review")]
    {
        commit(repo, &format!(".claude/skills/{name}/SKILL.md"), &skill_file(name));
    }
    world.afk(&["check"]).ok();
}

#[test]
fn a_stage_and_a_patrol_answer_their_harness_and_a_name_that_is_none_is_a_fault() {
    // Named or not, each stage and each patrol answers the harness it runs.
    let named = FEATURE.replace("do: implement\n", "do: implement\n    harness: claude-code\n");
    let sweep = "```yaml\nby: dev\ndo: implement\nevery: 1h\nmax_open: 1\n```\n";
    let world =
        bound(&[("team.md", TEAM), ("workflows/feature.md", &named), ("patrols/sweep.md", sweep)]);
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["harness"], "claude-code");
    let stages = read["workflows"][0]["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 2);
    for stage in stages {
        assert_eq!(stage["harness"], "claude-code", "{stage}");
    }
    assert_eq!(read["patrols"][0]["harness"], "claude-code");

    // It is a key where `model` is one, and nowhere else.
    let feature = FEATURE
        .replace("do: implement\n", "do: implement\n    harness: aider\n")
        .replace("do: rule }", "do: rule, harness: claude-code }")
        .replace("do: land }", "do: land, harness: claude-code }");
    let signed = "```yaml\nstages:\n  - { name: sign, by: human, harness: claude-code, on: { pass: done } }\n```\n";
    let team = TEAM.replace(
        "```\n",
        "precedents: { by: dev, do: implement, max_open: 1, harness: claude-code }\n```\n",
    );
    let world = bound(&[
        ("team.md", &team),
        ("workflows/feature.md", &feature),
        ("workflows/signed.md", signed),
        ("patrols/sweep.md", &sweep.replace("every", "harness: Claude Code\nevery")),
    ]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(
        faults(&answer.json),
        [
            "team.md: precedents.harness",
            "workflows/feature.md: land.harness",
            "workflows/feature.md: stages[0].harness",
            "workflows/feature.md: stages[0].escalate.harness",
            "workflows/signed.md: stages[0].harness",
            "patrols/sweep.md: harness",
        ]
    );
    let error = answer.error();
    let said = "workflows/feature.md: stages[0].harness: `aider` is no harness; the harnesses are: \
                claude-code, pi, codex";
    assert!(error.contains(said), "{error}");

    // The config's is a fault with its file and key.
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\nharness: aider\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    let error = world.afk(&["check"]).error();
    assert!(error.contains("docs/agents/afkconvoy.md"), "{error}");
    assert!(error.contains("harness: `aider` is no harness"), "{error}");
}

#[test]
fn a_role_is_read_at_the_head_of_its_default_branch() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let dev = world.root.join("github/acme/dev");
    let before = world.afk(&["check"]).ok();
    // A skill on another branch is not the role's; one pushed to the default
    // branch is, from the next read on.
    git(&dev, &["checkout", "-q", "-b", "draft"]);
    commit(&dev, ".claude/skills/draft/SKILL.md", "---\nname: draft\ndescription: a draft\n---\n");
    git(&dev, &["checkout", "-q", "main"]);
    assert_eq!(world.afk(&["check"]).ok()["roles"], before["roles"]);
    let pushed = commit(
        &dev,
        ".claude/skills/plan/SKILL.md",
        "---\nname: plan\ndescription: a skill\n---\n",
    );
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
    // No topic explains that, so it names none: no `see`, not an empty one.
    let unbound = world.afk(&["check"]).json;
    assert_eq!(unbound.as_object().unwrap().keys().collect::<Vec<_>>(), ["error"]);
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
    assert_eq!(bound["lock_label"], "fleet-lock", "a key every repo has");
    assert_eq!(bound["intake_limit"], 50);
    assert_eq!(bound["harness"], "claude-code", "a config that names none means Claude Code");
    assert_eq!(config("team: acme/team\nharness: claude-code").ok()["harness"], "claude-code");
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
        ("intake_limit: 5", "intake_limit"),
        ("harness: claude-code", "harness: is read only in a bound repo"),
        ("team: acme/team\nharness: aider", "harness: `aider` is no harness"),
        ("team: acme/team\nharness: [claude-code]", "harness"),
    ] {
        let error = config(yaml).error();
        assert!(error.contains(key), "{yaml:?} → {error}");
    }
    // A fleet lock needs no team: an unbound repo reads its label too.
    let unbound = config("lock_label: stop").ok();
    assert_eq!((&unbound["lock_label"], &unbound["team"]), (&json!("stop"), &Value::Null));
    assert_eq!(config("concurrency: 1").ok()["lock_label"], "fleet-lock");
}

/// A workflow of one stage, which the role `fellow` performs.
const WARD: &str = "\
# Ward

```yaml
stages:
  - { name: treat, by: fellow, do: treat, on: { pass: done } }
```
";

/// A world whose GitHub holds one repo, `acme/hospital`, of `files`, with the
/// config of the fleet's checkout naming `team` as its team.
fn hospital(team: &str, files: &[(&str, &str)]) -> World {
    let world = World::new();
    world.repo("acme/hospital", files);
    let config =
        format!("```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: {team}\n```\n");
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    world
}

fn team_file(roles: &str) -> String {
    format!("# The team\n\n```yaml\nroles: {roles}\n```\n")
}

const TREAT: &str = "roles/fellow/.claude/skills/treat/SKILL.md";
const TREATS: &str = "---\nname: treat\ndescription: Treat it.\n---\n";

#[test]
fn a_team_and_its_roles_in_one_repo_are_read_from_one_clone_of_it() {
    let team = team_file("{ fellow: ./roles/fellow }");
    let world = hospital(
        "acme/hospital",
        &[
            ("team.md", &team),
            ("workflows/ward.md", WARD),
            (TREAT, TREATS),
            ("roles/fellow/CLAUDE.md", "the rules of a fellow\n"),
            ("README.md", "for whoever reads the repo\n"),
        ],
    );
    let answer = world.afk(&["check"]);
    let commit = head(&world, "acme/hospital");
    assert_eq!(answer.stderr, "");
    let read = answer.ok();
    assert_eq!((&read["team"], &read["path"]), (&json!("acme/hospital"), &json!("")));
    assert_eq!(
        read["roles"],
        json!([{"name": "fellow", "repo": "acme/hospital", "path": "roles/fellow",
            "commit": commit, "host_user": true, "skills": ["treat"], "above": [],
            "limits": []}])
    );
    // The team's commit is its roles': the repo was read once.
    assert_eq!(read["commit"], commit);
    assert_eq!(clones(&world), ["acme/hospital"]);
}

#[test]
fn a_role_is_named_by_repo_by_repo_and_path_and_twice_over() {
    let team = team_file(
        "{ fellow: ./roles/fellow, night: acme/hospital/roles/fellow, dev: acme/dev, \
         boss: acme/roles/senior/boss }",
    );
    let world = hospital("acme/hospital", &[("team.md", &team), (TREAT, TREATS)]);
    world.repo(
        "acme/dev",
        &[(
            ".claude/skills/implement/SKILL.md",
            "---\nname: implement\ndescription: a skill\n---\n",
        )],
    );
    world.repo(
        "acme/roles",
        &[(
            "senior/boss/.claude/skills/rule/SKILL.md",
            "---\nname: rule\ndescription: a skill\n---\n",
        )],
    );
    let read = world.afk(&["check"]).ok();
    let role = |name: &str, repo: &str, path: &str, skill: &str| {
        json!({"name": name, "repo": repo, "path": path, "commit": head(&world, repo),
            "host_user": true, "skills": [skill], "above": [], "limits": []})
    };
    assert_eq!(
        read["roles"],
        json!([
            // Two names for one directory are two roles.
            role("fellow", "acme/hospital", "roles/fellow", "treat"),
            role("night", "acme/hospital", "roles/fellow", "treat"),
            role("dev", "acme/dev", "", "implement"),
            role("boss", "acme/roles", "senior/boss", "rule"),
        ])
    );
    assert_eq!(clones(&world), ["acme/dev", "acme/hospital", "acme/roles"]);
}

#[test]
fn a_team_in_a_subdirectory_reads_a_path_from_the_root_of_its_repo() {
    let team = team_file("{ fellow: ./roles/fellow }");
    let files = [
        ("teams/hospital/team.md", team.as_str()),
        ("teams/hospital/workflows/ward.md", WARD),
        (TREAT, TREATS),
    ];
    let world = hospital("acme/hospital/teams/hospital", &files);
    let read = world.afk(&["check"]).ok();
    assert_eq!((&read["team"], &read["path"]), (&json!("acme/hospital"), &json!("teams/hospital")));
    assert_eq!(read["files"], json!(["team.md", "workflows/ward.md"]));
    assert_eq!(read["roles"][0]["path"], "roles/fellow");
    assert_eq!(read["roles"][0]["skills"], json!(["treat"]));
    assert_eq!(read["workflows"][0]["stages"][0]["by"], "fellow");
    let named = world.afk(&["check", "--team", "acme/hospital/teams/hospital/"]).ok();
    assert_eq!(named["roles"], read["roles"]);
    assert_eq!(world.afk(&["config"]).ok()["team"], "acme/hospital/teams/hospital");
    // A team's directory that is none at the head is said so.
    let error = world.afk(&["check", "--team", "acme/hospital/teams/clinic"]).error();
    assert!(
        error.starts_with(
            "the team acme/hospital/teams/clinic cannot be read: teams/clinic is no directory at \
             the head of the default branch of acme/hospital"
        ),
        "{error}"
    );
}

#[test]
fn a_role_s_directory_at_fault_is_reported_with_its_key() {
    let team = team_file(
        "{ fellow: ./roles/fellow, gone: ./roles/gone, up: ./roles/../.., root: /roles/fellow, \
         all: ./, own: ./teams/night, inner: ./roles/fellow/night, linked: ./roles/linked, \
         elsewhere: acme/other/roles/x }",
    );
    let world = hospital(
        "acme/hospital/teams/night",
        &[("teams/night/team.md", &team), (TREAT, TREATS), ("shared/guard.sh", "exit 0\n")],
    );
    world.repo("acme/other", &[("README.md", "no roles here\n")]);
    let repo = world.root.join("github/acme/hospital");
    let linked = repo.join("roles/linked/hooks");
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink("../../../shared/guard.sh", linked.join("guard.sh")).unwrap();
    std::os::unix::fs::symlink("../CLAUDE.md", linked.join("rules.md")).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", linked.join("hosts")).unwrap();
    write(&repo, "roles/linked/CLAUDE.md", "the rules\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "links"]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    let commit = head(&world, "acme/hospital");
    let said: Vec<String> = answer.json["faults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| format!("{}: {}", f["key"].as_str().unwrap(), f["problem"].as_str().unwrap()))
        .collect();
    let hospital = "acme/hospital";
    assert_eq!(
        said,
        [
            format!(
                "roles.gone: {hospital}/roles/gone cannot be read: it is no directory at the \
                 head of the default branch of {hospital} ({commit})"
            ),
            "roles.up: `./roles/../..` has a `..`: a role's path is written from the root of \
             its repo, and names nothing outside it"
                .to_string(),
            "roles.root: `/roles/fellow` is an absolute path: a role is named as owner/repo, \
             owner/repo/path or ./path, its path written from the root of its repo"
                .to_string(),
            format!(
                "roles.all: its directory {hospital} contains the team's ({hospital}/teams/night)"
            ),
            format!(
                "roles.all: its directory {hospital} contains that of the role `fellow` \
                 ({hospital}/roles/fellow)"
            ),
            format!(
                "roles.all: its directory {hospital} contains that of the role `gone` \
                 ({hospital}/roles/gone)"
            ),
            format!(
                "roles.own: its directory is the team's own ({hospital}/teams/night): a role is \
                 a directory of its own"
            ),
            format!(
                "roles.inner: its directory {hospital}/roles/fellow/night is inside that of the \
                 role `fellow` ({hospital}/roles/fellow)"
            ),
            // A link that stays in the role is none of them.
            format!(
                "roles.linked: {hospital}/roles/linked holds a symlink that leaves it: \
                 hooks/guard.sh -> ../../../shared/guard.sh, hooks/hosts -> /etc/hosts. A worker \
                 is given the role's directory alone, where such a link points at nothing"
            ),
            format!(
                "roles.elsewhere: acme/other/roles/x cannot be read: it is no directory at the \
                 head of the default branch of acme/other ({})",
                head(&world, "acme/other")
            ),
        ]
    );
    assert!(answer.error().starts_with("the team acme/hospital/teams/night has 10 faults"));
}

#[test]
fn rules_above_a_role_are_noted_and_the_team_loads() {
    let team = team_file("{ fellow: ./roles/fellow, dev: acme/dev }");
    let world = hospital(
        "acme/hospital",
        &[
            ("team.md", &team),
            (TREAT, TREATS),
            ("CLAUDE.md", "how the hospital works\n"),
            ("roles/CLAUDE.md", "for whoever edits the roles\n"),
            ("roles/.claude/settings.json", "{}\n"),
        ],
    );
    // A whole repo has nothing above it.
    world.repo("acme/dev", &[("CLAUDE.md", "the rules of a dev\n")]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 0, "{}", answer.json);
    // The `CLAUDE.md` at the root is the team rules, which a worker is
    // given: it is none of what is above the role and not given.
    assert_eq!(answer.json["rules"], true);
    assert_eq!(answer.json["roles"][0]["above"], json!(["roles/CLAUDE.md", "roles/.claude/"]));
    assert_eq!(answer.json["roles"][1]["above"], json!([]));
    assert_eq!(
        answer.stderr.trim_end(),
        "note: above the role `fellow` (acme/hospital/roles/fellow), its repo holds \
         roles/CLAUDE.md, roles/.claude/: a session opened by hand in the role's directory loads \
         that too, and a worker does not — it is given the role's directory alone"
    );
}

#[test]
fn a_team_at_a_repo_s_root_is_given_no_note_for_its_own_rules() {
    let team = team_file("{ fellow: ./roles/fellow }");
    let files = [("team.md", team.as_str()), (TREAT, TREATS), ("CLAUDE.md", "how we work\n")];
    let world = hospital("acme/hospital", &files);
    let answer = world.afk(&["check"]);
    assert_eq!((answer.code, answer.stderr.as_str()), (0, ""), "{}", answer.json);
    assert_eq!(answer.json["rules"], true);
    assert_eq!(answer.json["roles"][0]["above"], json!([]));
    // The rules of another team's directory, above the role, are none of
    // this team's: its workers are not given them.
    let night = [("teams/night/team.md", team.as_str()), ("teams/night/CLAUDE.md", "at night\n")];
    let world = hospital("acme/hospital/teams/night", &[&files[1..], &night[..]].concat());
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 0, "{}", answer.json);
    assert_eq!(answer.json["rules"], true);
    assert_eq!(answer.json["roles"][0]["above"], json!(["CLAUDE.md"]));
}

#[test]
fn hooks_and_skills_in_a_team_s_directory_are_noted_and_the_team_loads() {
    let team = team_file("{ dev: acme/dev }");
    let world = hospital(
        "acme/hospital",
        &[
            ("team.md", &team),
            (
                ".claude/skills/release/SKILL.md",
                "---\nname: release\ndescription: a human's\n---\n",
            ),
        ],
    );
    world.repo("acme/dev", &[("CLAUDE.md", "the rules of a dev\n")]);
    let note = "note: the team's directory holds a .claude/: no worker loads the hooks and skills \
                there. Of a team's directory a worker is given its rules (CLAUDE.md); a hook and \
                a skill are a role's, in the role's own directory";
    let answer = world.afk(&["check"]);
    assert_eq!((answer.code, answer.stderr.trim_end()), (0, note), "{}", answer.json);
    assert_eq!(answer.json["rules"], false);
    // As its files in a directory, too.
    let edit = world.root.join("github/acme/hospital");
    let answer = world.afk(&["check", "--dir", edit.to_str().unwrap()]);
    assert_eq!((answer.code, answer.stderr.trim_end()), (0, note), "{}", answer.json);
}

#[test]
fn a_directory_of_a_team_s_files_names_its_repo_to_read_a_path_in_it() {
    let team = team_file("{ fellow: ./roles/fellow }");
    let world = hospital("acme/hospital", &[("team.md", &team), (TREAT, TREATS)]);
    // An edit of the team, not pushed: a new workflow.
    let edit = world.root.join("edit");
    write(&edit, "team.md", &team);
    write(&edit, "workflows/ward.md", WARD);
    let dir = edit.to_str().unwrap();
    let read = world.afk(&["check", "--dir", dir, "--team", "acme/hospital"]).ok();
    assert_eq!(
        (&read["team"], &read["path"], &read["commit"]),
        (&json!(dir), &json!(null), &json!(null))
    );
    assert_eq!(read["roles"][0]["commit"], head(&world, "acme/hospital"));
    assert_eq!(read["workflows"][0]["name"], "ward");
    // A directory says no repo.
    let answer = world.afk(&["check", "--dir", dir]);
    assert_eq!(faults(&answer.json), ["team.md: roles.fellow"]);
    let error = answer.error();
    assert!(error.contains("nothing says which repo these files are of"), "{error}");
}

/// One hook, as a role's settings for Claude Code hold a limit.
const HOOKED: &str = r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": []}]}}"#;

/// `FEATURE`, its first stage with `keys` — each a line — after its `do`.
fn building(keys: &[&str]) -> String {
    let keys: String = keys.iter().map(|key| format!("    {key}\n")).collect();
    FEATURE.replace("do: implement\n", &format!("do: implement\n{keys}"))
}

fn dev(world: &World) -> std::path::PathBuf {
    world.root.join("github/acme/dev")
}

#[test]
fn a_role_whose_files_would_have_pi_or_codex_load_other_rules_is_a_fault_naming_the_file() {
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    commit(&dev(&world), "CLAUDE.md", "the rules\n");
    world.afk(&["check"]).ok();
    // pi and Codex load `AGENTS.md` first, and then no `CLAUDE.md`: whatever
    // harness the role's stages run on today. The file is one fault.
    commit(&dev(&world), "AGENTS.md", "other rules\n");
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["team.md: roles.dev"]);
    let error = answer.error();
    assert!(
        error.contains(
            "team.md: roles.dev: acme/dev holds `AGENTS.md`: pi and Codex load that in place of \
             `CLAUDE.md`, the rules every harness works under."
        ),
        "{error}"
    );
    // A stage on `codex` is refused for `AGENTS.override.md` too, which
    // Codex loads before either.
    let on_codex = building(&["harness: codex"]);
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &on_codex)]);
    commit(&dev(&world), "CLAUDE.md", "the rules\n");
    assert_eq!(world.afk(&["check"]).ok()["workflows"][0]["stages"][0]["harness"], "codex");
    commit(&dev(&world), "AGENTS.override.md", "other rules\n");
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["team.md: roles.dev"]);
    let error = answer.error();
    assert!(
        error.contains("acme/dev holds `AGENTS.override.md`: pi and Codex load that in place of"),
        "{error}"
    );
}

#[test]
fn a_stage_on_a_harness_its_role_holds_no_limits_for_while_it_holds_some_is_a_fault() {
    let on_pi = building(&["harness: pi"]);
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &on_pi)]);
    // A role with no limits at all is right for every harness: a skeleton's
    // settings name no hook.
    commit(&dev(&world), ".claude/settings.json", "{\n  \"hooks\": {}\n}\n");
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["roles"][0]["limits"], json!([]));
    assert_eq!(read["workflows"][0]["stages"][0]["harness"], "pi");
    // One that holds limits for Claude Code and none for pi is not right for
    // a stage on pi.
    commit(&dev(&world), ".claude/settings.json", HOOKED);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(faults(&answer.json), ["workflows/feature.md: stages[0].harness"]);
    let error = answer.error();
    assert!(
        error.contains(
            "the stage `build` of the workflow `feature` runs on `pi`, and its role `dev` holds \
             limits for `claude-code` (in its `.claude/settings.json`) and none for `pi`"
        ),
        "{error}"
    );
    // Once it holds them for pi too, in pi's form, it is.
    commit(&dev(&world), ".pi/extensions/limits.ts", "export default function () {}\n");
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["roles"][0]["limits"], json!(["claude-code", "pi"]));
    // And a role with limits for pi alone is right for pi alone: every stage,
    // consult and landing that runs it on the default is refused, at once.
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &on_pi)]);
    for role in ["acme/dev", "acme/reviewer"] {
        commit(&world.root.join("github").join(role), ".pi/extensions/limits.ts", "//\n");
    }
    let answer = world.afk(&["check"]);
    assert_eq!(
        faults(&answer.json),
        [
            "workflows/feature.md: stages[0].escalate.to",
            "workflows/feature.md: stages[1].harness",
            "workflows/feature.md: land.by",
        ]
    );
    let error = answer.error();
    assert!(error.contains("runs on `claude-code`, the repo's default, and its role"), "{error}");
    // Settings that cannot be read hold limits nobody can count on.
    commit(&dev(&world), ".claude/settings.json", "{ hooks");
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["team.md: roles.dev"]);
    let error = answer.error();
    assert!(
        error.contains("its limits for Claude Code, in .claude/settings.json, cannot be read"),
        "{error}"
    );
}

/// A role's limits for Codex are the hooks of its `.codex/hooks.json`: it holds
/// some when the file names a hook at some event, whichever event that is.
#[test]
fn a_role_holds_limits_for_codex_when_its_hooks_file_names_a_hook_at_some_event() {
    // Every worker of this repo is on Codex: it is the repo's default.
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
                  harness: codex\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    let limits = |world: &World| world.afk(&["check"]).ok()["roles"][0]["limits"].clone();
    // No file, and a file that names no hook — a skeleton's, or one whose
    // every event is empty — hold no limit.
    assert_eq!(limits(&world), json!([]));
    for none in ["{\n  \"hooks\": {}\n}\n", "{}", r#"{"hooks": {"PreToolUse": [], "Stop": []}}"#] {
        commit(&dev(&world), ".codex/hooks.json", none);
        assert_eq!(limits(&world), json!([]), "{none}");
    }
    // A hook at any one event is a limit.
    for event in ["PreToolUse", "PermissionRequest", "UserPromptSubmit", "Stop"] {
        let hooked = json!({"hooks": {"SessionStart": [], event: [
            {"hooks": [{"type": "command", "command": "sh .codex/hooks/refuse.sh"}]}]}});
        commit(&dev(&world), ".codex/hooks.json", &hooked.to_string());
        assert_eq!(limits(&world), json!(["codex"]), "{event}");
    }
    // A file that cannot be read as hooks is an error that names it, and so
    // is a directory where the file goes.
    let unread = |world: &World| {
        let answer = world.afk(&["check"]);
        assert_eq!(faults(&answer.json), ["team.md: roles.dev"]);
        let error = answer.error();
        let named = "its limits for Codex, in .codex/hooks.json, cannot be read";
        assert!(error.contains(named), "{error}");
        error
    };
    for wrong in ["{ hooks", r#"{"hooks": ["PreToolUse"]}"#, r#"{"hooks": {"Stop": "refuse.sh"}}"#]
    {
        commit(&dev(&world), ".codex/hooks.json", wrong);
        unread(&world);
    }
    git(&dev(&world), &["rm", "-q", ".codex/hooks.json"]);
    commit(&dev(&world), ".codex/hooks.json/refuse.sh", "exit 2\n");
    assert!(unread(&world).contains("it is a directory, not a file of hooks"));
}

/// A stage is never moved to Codex into fewer limits unseen: the message is
/// the one every other harness gets.
#[test]
fn a_stage_on_codex_whose_role_holds_limits_for_another_harness_and_none_for_codex_is_refused() {
    let on_codex = building(&["harness: codex"]);
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &on_codex)]);
    let refused = |held: &str| {
        let answer = world.afk(&["check"]);
        assert_eq!(answer.code, 1);
        assert_eq!(faults(&answer.json), ["workflows/feature.md: stages[0].harness"]);
        let error = answer.error();
        let said = format!(
            "the stage `build` of the workflow `feature` runs on `codex`, and its role `dev` \
             holds limits for {held} and none for `codex`: a worker there would be held to none \
             of them. Write the role's limits for `codex` in its `.codex/hooks.json`, in that \
             harness's own form, or run this on a harness the role holds limits for"
        );
        assert!(error.contains(&said), "{error}");
    };
    // Limits for Claude Code alone, with the skeleton of Codex's beside them.
    commit(&dev(&world), ".claude/settings.json", HOOKED);
    refused("`claude-code` (in its `.claude/settings.json`)");
    commit(&dev(&world), ".codex/hooks.json", "{\n  \"hooks\": {}\n}\n");
    refused("`claude-code` (in its `.claude/settings.json`)");
    // Limits for pi too: still none for Codex.
    commit(&dev(&world), ".pi/extensions/limits.ts", "export default function () {}\n");
    refused("`claude-code` (in its `.claude/settings.json`) and `pi` (in its `.pi/extensions`)");
    // Once it holds them for Codex, in Codex's form, the stage is right; the
    // stages on the default harness always were.
    commit(&dev(&world), ".codex/hooks.json", HOOKED);
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["roles"][0]["limits"], json!(["claude-code", "pi", "codex"]));
    assert_eq!(read["workflows"][0]["stages"][0]["harness"], "codex");
    // And a role with limits for Codex alone is right for Codex alone.
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &on_codex)]);
    commit(&dev(&world), ".codex/hooks.json", HOOKED);
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["workflows/feature.md: land.by"]);
    let error = answer.error();
    assert!(
        error.contains(
            "runs on `claude-code`, the repo's default, and its role `dev` holds limits for \
             `codex` (in its `.codex/hooks.json`) and none for `claude-code`"
        ),
        "{error}"
    );
}

#[test]
fn a_model_its_harness_does_not_know_is_a_fault_where_the_harness_can_say() {
    let named = |model: &str| building(&["harness: pi", &format!("model: {model}")]);
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &named("opus"))]);
    let team = world.root.join("github/acme/team");
    // pi could not be asked on this host: the model is passed as written.
    let answer = world.afk(&["check"]);
    assert!(
        answer.stderr.contains("note: pi could not be asked which models it knows on this host"),
        "{}",
        answer.stderr
    );
    assert_eq!(answer.ok()["workflows"][0]["stages"][0]["model"], "opus");
    // Asked, in the shell a worker gets and under the directory the fleet
    // keeps for it, pi lists the models it knows: `opus` is none of them.
    world.shell_models(&["openai-codex/gpt-5.5", "zai/glm-5.2"]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(faults(&answer.json), ["workflows/feature.md: stages[0].model"]);
    let error = answer.error();
    assert!(
        error.contains(
            "`opus` is no model pi knows on this host: it lists openai-codex/gpt-5.5, zai/glm-5.2."
        ),
        "{error}"
    );
    let kept = world.cache().join("afkconvoy/harness/pi");
    let asked = format!("export PI_CODING_AGENT_DIR='{}' && pi --list-models", kept.display());
    assert!(world.shell_asked().contains(&asked), "{:?}", world.shell_asked());
    // One it lists is known by its id, and a model of Claude Code's is passed
    // as written: Claude Code cannot say which it knows.
    let sweep =
        "```yaml\nby: dev\ndo: implement\nevery: 1h\nmodel: no-such-model\nmax_open: 1\n```\n";
    commit(&team, "patrols/sweep.md", sweep);
    commit(&team, "workflows/feature.md", &named("gpt-5.5:high"));
    let answer = world.afk(&["check"]);
    assert_eq!(answer.stderr, "");
    assert_eq!(answer.ok()["patrols"][0]["model"], "no-such-model");
    // A patrol's model is held to its harness as a stage's is.
    commit(&team, "patrols/sweep.md", &sweep.replace("model:", "harness: pi\nmodel:"));
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["patrols/sweep.md: model"]);
    assert_eq!(see(&answer.json["faults"][0]), "afkconvoy help patrols");
}

#[test]
fn a_repo_bound_to_a_team_whose_models_fit_another_default_harness_is_told_so() {
    // The team names models and no harness: it is written for one default.
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &building(&["model: opus"]))]);
    world.shell_models(&["openai-codex/gpt-5.5"]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.stderr, "");
    answer.ok();
    // A repo that binds it under pi is told, as a fault of its own config.
    let config =
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\nharness: pi\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(faults(&answer.json), ["docs/agents/afkconvoy.md: harness"]);
    // The topic of the team says which keys of a config bind a repo to one.
    assert_eq!(see(&answer.json["faults"][0]), "afkconvoy help team");
    let error = answer.error();
    for said in [
        "the team acme/team names models and no harness for them — `opus` (the stage `build` of \
         the workflow `feature`) — so each runs on `pi`, this repo's default",
        "it lists openai-codex/gpt-5.5.",
        "The team is written for another default harness",
    ] {
        assert!(error.contains(said), "{said:?} is not in {error}");
    }
    // The other way round Claude Code cannot say, and the model is passed as
    // written — with a warning where it is pi's whole name for one it lists.
    let team = world.root.join("github/acme/team");
    commit(&team, "workflows/feature.md", &building(&["model: openai-codex/gpt-5.5"]));
    world.afk(&["check"]).ok();
    write(&world.clone, "docs/agents/afkconvoy.md", &config.replace("harness: pi\n", ""));
    let answer = world.afk(&["check"]);
    assert!(
        answer.stderr.contains(
            "warning: the stage `build` of the workflow `feature` names the model \
             `openai-codex/gpt-5.5` and no harness, so it runs on the default harness, \
             `claude-code` — and `openai-codex/gpt-5.5` is `pi`'s name for a model"
        ),
        "{}",
        answer.stderr
    );
    answer.ok();
}

#[test]
fn a_model_named_for_codex_is_held_to_the_ones_codex_lists_on_this_host() {
    let named = |model: &str| building(&["harness: codex", &format!("model: {model}")]);
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", &named("opus"))]);
    let team = world.root.join("github/acme/team");
    // Codex could not be asked on this host: the model is passed as written.
    let answer = world.afk(&["check"]);
    assert!(
        answer.stderr.contains("note: Codex could not be asked which models it knows on this host"),
        "{}",
        answer.stderr
    );
    assert_eq!(answer.ok()["workflows"][0]["stages"][0]["model"], "opus");
    // Asked, in the shell a worker gets and under the directory the fleet
    // keeps for it, its worker launch command lists the models it was built
    // with: `opus` is none of them.
    world.shell_built_with(&["gpt-5.5", "gpt-6-sol"]);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(faults(&answer.json), ["workflows/feature.md: stages[0].model"]);
    let error = answer.error();
    assert!(
        error.contains("`opus` is no model Codex knows on this host: it lists gpt-5.5, gpt-6-sol."),
        "{error}"
    );
    let kept = world.cache().join("afkconvoy/harness/codex");
    let asked = format!(
        "export CODEX_HOME='{}' && {} debug models --bundled",
        kept.display(),
        afkconvoy_core::harness::Harness::Codex.stock()
    );
    assert!(world.shell_asked().contains(&asked), "{:?}", world.shell_asked());
    // One it lists is known by its whole name, and by no part of it.
    commit(&team, "workflows/feature.md", &named("gpt-6-sol"));
    let answer = world.afk(&["check"]);
    assert_eq!(answer.stderr, "");
    answer.ok();
    commit(&team, "workflows/feature.md", &named("gpt-6"));
    assert_eq!(faults(&world.afk(&["check"]).json), ["workflows/feature.md: stages[0].model"]);
    // A Codex that lists nothing has said nothing: the model is passed as
    // written.
    world.shell_built_with(&[]);
    let answer = world.afk(&["check"]);
    assert!(answer.stderr.contains("or listed none: a model named for it is passed as written"));
    assert_eq!(answer.ok()["workflows"][0]["stages"][0]["model"], "gpt-6");
}

#[test]
fn a_model_of_codex_s_named_with_no_harness_under_another_default_is_told_so() {
    let world =
        bound(&[("team.md", TEAM), ("workflows/feature.md", &building(&["model: gpt-6-sol"]))]);
    world.shell_built_with(&["gpt-5.5", "gpt-6-sol"]);
    // Claude Code cannot say which models it knows, and the model is passed
    // as written — with a warning: it is Codex's name for one it lists.
    let answer = world.afk(&["check"]);
    assert!(
        answer.stderr.contains(
            "warning: the stage `build` of the workflow `feature` names the model `gpt-6-sol` \
             and no harness, so it runs on the default harness, `claude-code` — and `gpt-6-sol` \
             is `codex`'s name for a model"
        ),
        "{}",
        answer.stderr
    );
    answer.ok();
    // pi can say, and lists no such model: a fault of the repo's own config.
    world.shell_models(&["zai/glm-5.2"]);
    let config = |harness: &str| {
        format!(
            "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
             harness: {harness}\n```\n"
        )
    };
    write(&world.clone, "docs/agents/afkconvoy.md", &config("pi"));
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["docs/agents/afkconvoy.md: harness"]);
    let error = answer.error();
    assert!(error.contains("`gpt-6-sol` (the stage `build` of the workflow `feature`)"), "{error}");
    assert!(error.contains("so each runs on `pi`, this repo's default"), "{error}");
    // Under Codex it is one Codex lists — and a model of another harness's
    // is the fault there.
    write(&world.clone, "docs/agents/afkconvoy.md", &config("codex"));
    let answer = world.afk(&["check"]);
    assert_eq!(answer.stderr, "");
    answer.ok();
    let team = world.root.join("github/acme/team");
    commit(&team, "workflows/feature.md", &building(&["model: zai/glm-5.2"]));
    let answer = world.afk(&["check"]);
    assert_eq!(faults(&answer.json), ["docs/agents/afkconvoy.md: harness"]);
    let error = answer.error();
    assert!(error.contains("so each runs on `codex`, this repo's default"), "{error}");
    assert!(error.contains("it lists gpt-5.5, gpt-6-sol."), "{error}");
}

/// What a model named for Codex is checked against is said where `check` is
/// described.
#[test]
fn the_help_of_check_says_what_a_model_named_for_codex_is_checked_against() {
    let world = World::new();
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root)
        .args(["check", "--help"])
        .output();
    let help = String::from_utf8(out.unwrap().stdout).unwrap();
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    let asked =
        format!("{} debug models --bundled", afkconvoy_core::harness::Harness::Codex.stock());
    for said in [asked.as_str(), "each by its `slug`", "Codex knows a model by the whole `slug`"] {
        assert!(help.contains(said), "check --help never says {said:?}");
    }
}

/// Where a role's limits for each harness go is said wherever a role's files
/// are: in the topic `team`, in `scaffold`'s help and in the rules a skeleton is
/// written with — beside what is written once.
#[test]
fn the_help_and_a_skeleton_say_where_a_roles_limits_for_each_harness_go() {
    use afkconvoy_core::harness::Harness;
    let world = World::new();
    let asked = |args: &[&str]| {
        let out = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root).args(args).output();
        String::from_utf8(out.unwrap().stdout).unwrap()
    };
    world.afk_in(&world.root, &["scaffold", "role", "dev"]).ok();
    let rules = std::fs::read_to_string(world.root.join("dev/CLAUDE.md")).unwrap();
    let said = [
        ("help team", asked(&["help", "team"])),
        ("scaffold --help", asked(&["scaffold", "--help"])),
        ("a skeleton's rules", rules),
    ];
    for (name, text) in &said {
        for harness in Harness::ALL {
            let limits = harness.limits();
            assert!(text.contains(limits), "{name} never says {limits}, for {}", harness.key());
        }
        assert!(text.contains("written once"), "{name} never says what is written once");
    }
    // `check` says where it looks for each harness's, and the skeleton holds
    // Codex's beside Claude Code's.
    let check = asked(&["check", "--help"]);
    for harness in Harness::ALL {
        assert!(check.contains(harness.limits()), "check --help: {}", harness.key());
    }
    assert!(world.root.join("dev").join(Harness::Codex.limits()).is_file());
}

#[test]
fn a_team_file_may_turn_the_host_user_s_skills_and_rules_off_with_true_or_false_alone() {
    let off = TEAM.replace("```\n", "host_user: false\n```\n");
    let world = bound(&[("team.md", &off), ("workflows/feature.md", FEATURE)]);
    assert_eq!(world.afk(&["check"]).ok()["host_user"], false);
    let team = world.root.join("github/acme/team");
    commit(&team, "team.md", &TEAM.replace("```\n", "host_user: \"no\"\n```\n"));
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(faults(&answer.json), ["team.md: host_user"]);
    let problem = answer.json["faults"][0]["problem"].as_str().unwrap();
    assert_eq!(problem, "must be true or false, not text");
    see(&answer.json["faults"][0]);
}

#[test]
fn a_role_named_with_its_dir_says_its_own_host_user_and_check_answers_each_role_s() {
    let own = TEAM
        .replace("reviewer: acme/reviewer", "reviewer: { dir: acme/reviewer, host_user: false }");
    let world = bound(&[("team.md", &own), ("workflows/feature.md", FEATURE)]);
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["host_user"], true);
    let said: Vec<_> = read["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].clone(), r["host_user"].clone()))
        .collect();
    assert_eq!(said, [(json!("dev"), json!(true)), (json!("reviewer"), json!(false))]);

    let team = world.root.join("github/acme/team");
    let wrong = TEAM.replace(
        "reviewer: acme/reviewer",
        "reviewer: { dir: acme/reviewer, host_user: 0, at: x }",
    );
    commit(&team, "team.md", &wrong);
    let answer = world.afk(&["check"]);
    assert_eq!(answer.code, 1);
    assert_eq!(
        faults(&answer.json),
        ["team.md: roles.reviewer.at", "team.md: roles.reviewer.host_user"]
    );
}

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
    assert_eq!((&read["team"], &read["path"]), (&json!("acme/team"), &json!("")));
    assert_eq!(read["commit"], head(&world, "acme/team"));
    assert_eq!(read["files"], json!(["team.md", "workflows/feature.md"]));
    assert_eq!(read["escalate_label"], "ready-for-human");
    assert_eq!(read["rules"], false, "the team holds no CLAUDE.md");
    assert_eq!(
        read["roles"],
        json!([
            {"name": "dev", "repo": "acme/dev", "path": "", "commit": head(&world, "acme/dev"),
             "skills": ["implement", "land"], "above": []},
            {"name": "reviewer", "repo": "acme/reviewer", "path": "",
             "commit": head(&world, "acme/reviewer"), "skills": ["review", "rule"], "above": []},
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
                 "model": null, "writes": false, "escalate": null, "pass": "land", "reject": "build"},
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
    let pushed = commit(&world.root.join("github/acme/dev"), ".claude/skills/plan/SKILL.md", "a\n");
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
        .replace("do: implement\n", "do: implement\n    harness: codex\n")
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
    let said = "workflows/feature.md: stages[0].harness: `codex` is no harness; the harnesses are: \
                claude-code, pi";
    assert!(error.contains(said), "{error}");

    // The config's is a fault with its file and key.
    let world = bound(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\nharness: codex\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    let error = world.afk(&["check"]).error();
    assert!(error.contains("docs/agents/afkconvoy.md"), "{error}");
    assert!(error.contains("harness: `codex` is no harness"), "{error}");
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
        ("team: acme/team\nharness: codex", "harness: `codex` is no harness"),
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

/// The help is where an agent learns a team's files: it names every key the
/// loader reads.
#[test]
fn the_help_names_every_key_of_a_definition() {
    use afkconvoy_core::team::{
        ESCALATE_KEYS, LAND_KEYS, ON_KEYS, PATROL_KEYS, PRECEDENTS_KEYS, STAGE_KEYS, TEAM_KEYS,
        WORKFLOW_KEYS,
    };
    let world = World::new();
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root)
        .args(["check", "--help"])
        .output();
    let help = String::from_utf8(out.unwrap().stdout).unwrap();
    let keys = [
        &TEAM_KEYS[..],
        &WORKFLOW_KEYS,
        &STAGE_KEYS,
        &ON_KEYS,
        &ESCALATE_KEYS,
        &LAND_KEYS,
        &PATROL_KEYS,
        &PRECEDENTS_KEYS,
    ];
    for key in keys.concat() {
        assert!(help.contains(&format!(" {key}:")), "`check --help` never shows the key {key}");
    }
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

#[test]
fn a_team_and_its_roles_in_one_repo_are_read_from_one_clone_of_it() {
    let team = team_file("{ fellow: ./roles/fellow }");
    let world = hospital(
        "acme/hospital",
        &[
            ("team.md", &team),
            ("workflows/ward.md", WARD),
            (TREAT, "a skill\n"),
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
            "commit": commit, "skills": ["treat"], "above": []}])
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
    let world = hospital("acme/hospital", &[("team.md", &team), (TREAT, "a skill\n")]);
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", "a skill\n")]);
    world.repo("acme/roles", &[("senior/boss/.claude/skills/rule/SKILL.md", "a skill\n")]);
    let read = world.afk(&["check"]).ok();
    let role = |name: &str, repo: &str, path: &str, skill: &str| {
        json!({"name": name, "repo": repo, "path": path, "commit": head(&world, repo),
            "skills": [skill], "above": []})
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
        (TREAT, "a skill\n"),
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
        &[("teams/night/team.md", &team), (TREAT, "a skill\n"), ("shared/guard.sh", "exit 0\n")],
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
            (TREAT, "a skill\n"),
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
    let files = [("team.md", team.as_str()), (TREAT, "a skill\n"), ("CLAUDE.md", "how we work\n")];
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
        &[("team.md", &team), (".claude/skills/release/SKILL.md", "a human's\n")],
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
    let world = hospital("acme/hospital", &[("team.md", &team), (TREAT, "a skill\n")]);
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

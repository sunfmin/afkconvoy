//! `scaffold`: the skeleton of a role and of a team are written, created on
//! GitHub when asked, and accepted by `check` exactly as written.

mod common;

use common::{World, git};
use serde_json::{Value, json};

#[test]
fn what_is_scaffolded_and_created_passes_check_unedited() {
    let world = World::new();
    let (role, team) = (world.root.join("made/dev"), world.root.join("made/team"));
    let (role_dir, team_dir) = (role.to_str().unwrap(), team.to_str().unwrap());

    let wrote = world.afk(&["scaffold", "role", role_dir, "--repo", "acme/dev"]).ok();
    assert_eq!(
        wrote,
        json!({"scaffolded": "role", "dir": role, "repo": "acme/dev", "files":
            ["CLAUDE.md", ".claude/skills/implement/SKILL.md", ".claude/settings.json"]})
    );
    // A role's hooks are Claude Code's settings, and its skill says what it is.
    let hooks = std::fs::read_to_string(role.join(".claude/settings.json")).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&hooks).unwrap(), json!({"hooks": {}}));
    let skill = std::fs::read_to_string(role.join(".claude/skills/implement/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: implement\ndescription: "), "{skill}");
    // The repo was created from the one commit that holds exactly the skeleton.
    assert_eq!(git(&role, &["status", "--porcelain"]), "");
    assert_eq!(git(&role, &["rev-list", "--count", "HEAD"]), "1");
    let created = ["repo", "create", "acme/dev", "--private", "--source", ".", "--push"];
    assert_eq!(world.gh_calls(), [created]);

    let wrote = world
        .afk(&["scaffold", "team", team_dir, "--role", "dev=acme/dev", "--repo", "acme/team"])
        .ok();
    assert_eq!(wrote["files"], json!(["team.md", "workflows/feature.md"]));
    world.gh_calls();

    // As files in a directory, and as the repo that was created.
    for from in [["--dir", team_dir], ["--team", "acme/team"]] {
        let read = world.afk(&[&["check"][..], &from].concat()).ok();
        assert_eq!(read["files"], wrote["files"]);
        assert_eq!(read["roles"][0]["name"], "dev");
        assert_eq!(read["roles"][0]["skills"], json!(["implement"]));
        let feature = &read["workflows"][0];
        assert_eq!(feature["flow_label"], "flow:feature");
        assert_eq!(feature["stages"][0]["do"], "implement");
        assert_eq!(feature["land"], json!({"by": "dev", "do": "implement"}));
    }
    // And the lines the skeleton itself names run on it.
    assert!(
        world.afk(&["rehearse", "--dir", team_dir, "feature", "pass"]).ok()["ends"].is_object()
    );
}

#[test]
fn without_a_repo_it_only_writes_files_and_never_over_anything() {
    let world = World::new();
    let dir = world.root.join("role");
    let at = dir.to_str().unwrap();
    std::fs::create_dir(&dir).unwrap();
    let wrote = world.afk(&["scaffold", "role", at]).ok();
    assert_eq!((&wrote["repo"], &wrote["dir"]), (&Value::Null, &json!(dir)));
    assert!(!dir.join(".git").exists());
    assert!(world.gh_calls().is_empty());
    // What is there is left as it is.
    std::fs::write(dir.join("CLAUDE.md"), "mine\n").unwrap();
    for what in [&["role"][..], &["team", "--role", "dev=acme/dev"]] {
        let error = world.afk(&[&["scaffold"], what, &[at]].concat()).error();
        assert!(error.contains("is not empty"), "{error}");
    }
    assert_eq!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), "mine\n");
}

#[test]
fn a_skeleton_that_cannot_be_asked_for_or_created_is_an_error() {
    let world = World::new();
    let at = |name: &str| world.root.join(name).to_str().unwrap().to_string();
    let error = world.afk(&["scaffold", "team", &at("a")]).error();
    assert!(error.contains("--role <name>=<owner/repo>"), "{error}");
    let error = world.afk(&["scaffold", "role", &at("a"), "--role", "dev=acme/dev"]).error();
    assert!(error.contains("--role is a team's"), "{error}");
    assert!(!world.root.join("a").exists(), "nothing is written for either");
    for wrong in [&["--role", "dev"][..], &["--repo", "acme"], &["--public"]] {
        let answer = world.afk(&[&["scaffold", "team", &at("a")], wrong].concat());
        assert_eq!(answer.code, 2, "{wrong:?}: {}", answer.json);
    }
    // A repo GitHub refuses to create: the files stay, and the error says so.
    world.repo("acme/dev", &[]);
    let error = world.afk(&["scaffold", "role", &at("b"), "--repo", "acme/dev"]).error();
    assert!(error.contains("its repo is not created"), "{error}");
    assert!(error.contains("gh repo create failed"), "{error}");
    assert!(world.root.join("b/CLAUDE.md").is_file());
}

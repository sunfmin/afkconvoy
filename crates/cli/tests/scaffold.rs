//! `scaffold`: the skeleton of a team with its roles, and of one role, are
//! written, created on GitHub when asked, and accepted by `check` exactly as
//! written.

mod common;

use common::{World, git};
use serde_json::{Value, json};

const ROLE_FILES: [&str; 3] =
    ["CLAUDE.md", ".claude/skills/implement/SKILL.md", ".claude/settings.json"];

#[test]
fn a_team_and_its_roles_in_one_repo_pass_check_after_the_one_push() {
    let world = World::new();
    let team = world.root.join("made/hospital");
    let dir = team.to_str().unwrap();
    let wrote = world
        .afk(&[
            "scaffold",
            "team",
            dir,
            "--role",
            "fellow",
            "--role",
            "attending",
            "--repo",
            "acme/hospital",
        ])
        .ok();
    let held = |role: &str| ROLE_FILES.map(|file| format!("roles/{role}/{file}"));
    let files = ["team.md", "CLAUDE.md", "workflows/feature.md"].map(str::to_string);
    let files = [files.to_vec()];
    let files = [&files[..], &[held("fellow").to_vec(), held("attending").to_vec()]].concat();
    assert_eq!(
        wrote,
        json!({"scaffolded": "team", "dir": team, "repo": "acme/hospital",
            "files": files.concat(), "team_line": null})
    );
    let file = std::fs::read_to_string(team.join("team.md")).unwrap();
    assert!(file.contains("roles:\n  fellow: ./roles/fellow\n  attending: ./roles/attending\n"));
    // Its rules: a heading, and two lines on what belongs there and what
    // belongs in a repo's own rules or a role's.
    let rules = std::fs::read_to_string(team.join("CLAUDE.md")).unwrap();
    let lines: Vec<&str> = rules.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 3, "{rules}");
    assert_eq!(lines[0], "# Team rules");
    assert!(lines[1].starts_with("Here: how this team works, in every repo bound to it"));
    assert!(lines[2].contains("that repo's own rules") && lines[2].contains("its role's"));
    // One repo was created, from the one commit that holds exactly the skeleton.
    assert_eq!(git(&team, &["status", "--porcelain"]), "");
    assert_eq!(git(&team, &["rev-list", "--count", "HEAD"]), "1");
    let created = ["repo", "create", "acme/hospital", "--private", "--source", ".", "--push"];
    assert_eq!(world.gh_calls(), [created]);

    // As the repo that was created, and as its files in their directory.
    for from in [&["--team", "acme/hospital"][..], &["--dir", dir, "--team", "acme/hospital"]] {
        let read = world.afk(&[&["check"], from].concat()).ok();
        let roles = read["roles"].as_array().unwrap();
        for (role, name) in roles.iter().zip(["fellow", "attending"]) {
            assert_eq!(
                (&role["name"], &role["repo"], &role["path"], &role["skills"], &role["above"]),
                (
                    &json!(name),
                    &json!("acme/hospital"),
                    &json!(format!("roles/{name}")),
                    &json!(["implement"]),
                    &json!([])
                )
            );
        }
        assert_eq!(roles.len(), 2);
        // The team's own rules are the skeleton's, and above no role.
        assert_eq!(read["rules"], true);
        // The example workflow asks the first role for the skill its skeleton holds.
        let feature = &read["workflows"][0];
        assert_eq!(
            (&feature["stages"][0]["by"], &feature["stages"][0]["do"]),
            (&json!("fellow"), &json!("implement"))
        );
        assert_eq!(feature["land"], json!({"by": "fellow", "do": "implement"}));
    }
    // And the lines the skeleton itself names run on it, as it writes them.
    for file in ["team.md", "workflows/feature.md"] {
        let text = std::fs::read_to_string(team.join(file)).unwrap();
        assert!(text.contains(" --dir . --team acme/hospital"), "{file}: {text}");
    }
    let rehearse = ["rehearse", "--dir", ".", "--team", "acme/hospital", "feature", "pass"];
    assert!(world.afk_in(&team, &rehearse).ok()["ends"].is_object());
}

#[test]
fn a_team_names_a_role_written_here_and_one_that_is_elsewhere() {
    let world = World::new();
    let team = world.root.join("team");
    let dir = team.to_str().unwrap();
    let roles = ["--role", "a", "--role", "b=acme/b", "--role", "c=acme/shared/roles/c"];
    let wrote = world.afk(&[&["scaffold", "team", dir][..], &roles].concat()).ok();
    let files: Vec<&str> =
        wrote["files"].as_array().unwrap().iter().flat_map(Value::as_str).collect();
    assert_eq!(files[..3], ["team.md", "CLAUDE.md", "workflows/feature.md"]);
    assert!(files[3..].iter().all(|file| file.starts_with("roles/a/")), "{files:?}");
    assert_eq!(std::fs::read_dir(team.join("roles")).unwrap().count(), 1);
    let file = std::fs::read_to_string(team.join("team.md")).unwrap();
    assert!(
        file.contains("roles:\n  a: ./roles/a\n  b: acme/b\n  c: acme/shared/roles/c\n```"),
        "{file}"
    );
    // No repo was said: the lines it teaches leave the team's to be filled in.
    assert!(file.contains("afkconvoy check --dir . --team <owner/repo>"), "{file}");
    assert!(world.gh_calls().is_empty());

    // A team written in a checkout is a directory of that repo, and a role's
    // path is read from the repo's root.
    let night = world.clone.join("teams/night");
    world.afk(&["scaffold", "team", night.to_str().unwrap(), "--role", "a"]).ok();
    let file = std::fs::read_to_string(night.join("team.md")).unwrap();
    assert!(file.contains("  a: ./teams/night/roles/a\n"), "{file}");
    assert!(file.contains("--team <owner/repo>/teams/night"), "{file}");
    assert!(night.join("roles/a/CLAUDE.md").is_file());
}

#[test]
fn a_role_written_in_a_checkout_says_what_to_add_to_the_team_file() {
    let world = World::new();
    let wrote = world.afk(&["scaffold", "role", "roles/reviewer"]).ok();
    let dir = world.clone.join("roles/reviewer");
    assert_eq!(
        wrote,
        json!({"scaffolded": "role", "dir": dir, "repo": null, "files": ROLE_FILES,
            "team_line": "reviewer: ./roles/reviewer"})
    );
    assert!(dir.join("CLAUDE.md").is_file());
    // Nothing is created on GitHub, and nothing is committed.
    assert!(world.gh_calls().is_empty());
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "?? roles/");
    // From a directory of the checkout, the path is still the repo's.
    let wrote = world.afk_in(&world.clone.join("roles"), &["scaffold", "role", "night"]).ok();
    assert_eq!(wrote["team_line"], "night: ./roles/night");
}

#[test]
fn a_role_in_a_repo_of_its_own_is_created_and_a_team_names_it() {
    let world = World::new();
    let (role, team) = (world.root.join("made/dev"), world.root.join("made/team"));
    let (role_dir, team_dir) = (role.to_str().unwrap(), team.to_str().unwrap());

    let wrote = world.afk(&["scaffold", "role", role_dir, "--repo", "acme/dev"]).ok();
    assert_eq!(
        wrote,
        json!({"scaffolded": "role", "dir": role, "repo": "acme/dev", "files": ROLE_FILES,
            "team_line": "dev: acme/dev"})
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
    assert_eq!(wrote["files"], json!(["team.md", "CLAUDE.md", "workflows/feature.md"]));
    world.gh_calls();

    // As files in a directory, and as the repo that was created.
    for from in [["--dir", team_dir], ["--team", "acme/team"]] {
        let read = world.afk(&[&["check"][..], &from].concat()).ok();
        assert_eq!(read["files"], json!(["team.md", "workflows/feature.md"]));
        assert_eq!(read["rules"], true);
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
    // In no checkout, nothing says what a team would name it by.
    assert_eq!(wrote["team_line"], Value::Null);
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
    assert!(error.contains("needs --role <name>"), "{error}");
    let error = world.afk(&["scaffold", "team", &at("a"), "--role", "dev", "--role", "dev=a/b"]);
    assert!(error.error().contains("names `dev` twice"));
    let error = world.afk(&["scaffold", "role", &at("a"), "--role", "dev=acme/dev"]).error();
    assert!(error.contains("--role is a team's"), "{error}");
    assert!(!world.root.join("a").exists(), "nothing is written for either");
    for wrong in
        [&["--role", "Dev"][..], &["--role", "dev=acme"], &["--repo", "acme"], &["--public"]]
    {
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

/// What a harness's hook is handed for a `Bash` call of `command`.
fn bash_call(command: &str) -> String {
    json!({"tool_name": "Bash", "tool_input": {"command": command}}).to_string()
}

/// Runs every hook the role in `dir` has Claude Code run before a `Bash`
/// call, as Claude Code does, on `command` → each one's exit code.
fn hooked(dir: &std::path::Path, command: &str) -> Vec<Option<i32>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let settings = std::fs::read_to_string(dir.join(".claude/settings.json")).unwrap();
    let settings: Value = serde_json::from_str(&settings).unwrap();
    let before = settings["hooks"]["PreToolUse"].as_array().cloned().unwrap_or_default();
    let hooks = before.iter().flat_map(|entry| entry["hooks"].as_array().unwrap());
    let run = |hook: &Value| {
        let line = hook["command"].as_str().unwrap();
        let mut sh = Command::new("sh");
        sh.args(["-c", line]).env("CLAUDE_PROJECT_DIR", dir).current_dir(dir);
        let mut child = sh.stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(bash_call(command).as_bytes()).unwrap();
        child.wait_with_output().unwrap().status.code()
    };
    hooks.map(run).collect()
}

#[test]
fn the_example_team_is_written_whole_and_passes_check_as_pushed() {
    let world = World::new();
    let team = world.root.join("made/hospital");
    let dir = team.to_str().unwrap();
    let hospital = afkconvoy::scaffold::example("hospital").unwrap();
    let wrote =
        world.afk(&["scaffold", "team", dir, "--example", "hospital", "--repo", "acme/h"]).ok();
    let files: Vec<&str> = hospital.files.iter().map(|(path, _)| *path).collect();
    assert_eq!(
        wrote,
        json!({"scaffolded": "team", "dir": team, "repo": "acme/h", "files": files,
            "team_line": null})
    );
    // Each file is there as the binary holds it, and nothing else is.
    for (path, text) in hospital.files {
        assert_eq!(&std::fs::read_to_string(team.join(path)).unwrap(), text, "{path}");
    }
    assert_eq!(git(&team, &["status", "--porcelain"]), "");
    let tracked = git(&team, &["ls-files"]);
    assert_eq!(tracked.lines().count(), files.len(), "{tracked}");
    let created = ["repo", "create", "acme/h", "--private", "--source", ".", "--push"];
    assert_eq!(world.gh_calls(), [created]);

    // The team's loader takes it as pushed, with no edit: seven roles in the
    // team's own repo, three workflows, four patrols, rules and precedents.
    let read = world.afk(&["check", "--team", "acme/h"]).ok();
    let names = |of: &str, key: &str| -> Vec<String> {
        let all = read[of].as_array().unwrap_or_else(|| panic!("{of}: {read}")).iter();
        all.map(|one| one[key].as_str().unwrap().to_string()).collect()
    };
    let roles = names("roles", "name");
    assert_eq!(roles.len(), 7, "{roles:?}");
    for role in read["roles"].as_array().unwrap() {
        let name = role["name"].as_str().unwrap();
        assert_eq!(
            (&role["repo"], &role["path"], &role["above"]),
            (&json!("acme/h"), &json!(format!("roles/{name}")), &json!([])),
        );
        assert!(!role["skills"].as_array().unwrap().is_empty(), "{role}");
    }
    assert_eq!(names("workflows", "name"), ["chore", "design", "fix"]);
    assert_eq!(names("patrols", "name"), ["research", "rounds", "safety-review", "triage"]);
    assert_eq!(read["rules"], true);
    assert_eq!(read["precedents"]["by"], "safety");
    // The two roles with hooks hold limits, and the rest hold none.
    let limited: Vec<&str> = read["roles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|role| role["limits"] != json!([]))
        .map(|role| role["name"].as_str().unwrap())
        .collect();
    assert_eq!(limited, ["fellow", "attending"]);
    // And an issue is walked through its longest workflow.
    let walk = ["rehearse", "--team", "acme/h", "fix", "pass", "pass", "pass", "pass", "pass"];
    assert!(world.afk(&walk).ok()["ends"].is_object());
}

#[test]
fn the_example_teams_hooks_are_executable_and_refuse_what_the_rules_say() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let team = world.root.join("hospital");
    world.afk(&["scaffold", "team", team.to_str().unwrap(), "--example", "hospital"]).ok();
    // A file a harness runs is written executable, and no other is.
    let hospital = afkconvoy::scaffold::example("hospital").unwrap();
    let mut programs = Vec::new();
    for (path, _) in hospital.files {
        let mode = std::fs::metadata(team.join(path)).unwrap().permissions().mode();
        assert_eq!(mode & 0o111 != 0, path.contains("/hooks/"), "{path}: {mode:o}");
        programs.extend(path.contains("/hooks/").then_some(*path));
    }
    assert_eq!(programs.len(), 4, "{programs:?}");
    // Every role's hooks run, and let through what no rule is about.
    for role in std::fs::read_dir(team.join("roles")).unwrap() {
        let role = role.unwrap().path();
        let ran = hooked(&role, "git status");
        assert!(ran.iter().all(|code| *code == Some(0)), "{}: {ran:?}", role.display());
    }
    // What each role's rules say it never does is refused by a hook of its own.
    for (role, never, refused) in [
        (
            "fellow",
            "`git commit --no-verify`, a forced push, a rebase",
            &[
                "git commit --no-verify -m x",
                "git push --force origin HEAD",
                "git push --force-with-lease",
                "git rebase main",
            ][..],
        ),
        ("attending", "committing, and pushing", &["git commit -m x", "git push origin HEAD"]),
    ] {
        let dir = team.join("roles").join(role);
        let rules = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        let said = rules.split("\n## What you never do\n").nth(1).expect("the section");
        assert!(said.contains(&format!("refused by a hook, whatever you were asked: {never}.")));
        assert_eq!(hooked(&dir, "git status").len(), 2, "{role}");
        for command in refused {
            let ran = hooked(&dir, command);
            assert!(ran.contains(&Some(2)), "{role} is not refused `{command}`: {ran:?}");
        }
    }
    // A fellow commits and pushes: only the attending is refused those.
    assert!(!hooked(&team.join("roles/fellow"), "git commit -m x").contains(&Some(2)));
}

#[test]
fn an_example_is_a_team_whole_and_its_name_is_one_there_is() {
    let world = World::new();
    let at = |name: &str| world.root.join(name).to_str().unwrap().to_string();
    // A name there is no example for is refused, with the names there are.
    let answer = world.afk(&["scaffold", "team", &at("a"), "--example", "clinic"]);
    assert_eq!(answer.code, 2);
    let error = answer.error();
    assert!(error.contains("invalid value 'clinic' for '--example"), "{error}");
    let names = afkconvoy::scaffold::example_names();
    assert_eq!(names, ["hospital"]);
    assert!(error.contains(&format!("[possible values: {}]", names.join(", "))), "{error}");
    let error =
        world.afk(&["scaffold", "team", &at("a"), "--example", "hospital", "--role", "x"]).error();
    assert!(error.contains("--example names its own roles"), "{error}");
    let error = world.afk(&["scaffold", "role", &at("a"), "--example", "hospital"]).error();
    assert!(error.contains("--example is a team's"), "{error}");
    assert!(!world.root.join("a").exists(), "nothing is written for any of them");

    // Written in a checkout, the team is a directory of that repo, and each
    // role's path is read from the repo's root.
    let night = world.clone.join("teams/night");
    world.afk(&["scaffold", "team", night.to_str().unwrap(), "--example", "hospital"]).ok();
    let file = std::fs::read_to_string(night.join("team.md")).unwrap();
    assert!(file.contains("\n  fellow: ./teams/night/roles/fellow\n"), "{file}");
    assert!(!file.contains(": ./roles/"), "{file}");
    assert!(night.join("roles/fellow/CLAUDE.md").is_file());
    assert!(world.gh_calls().is_empty());
}

/// The files the topic names: the words of its lines four spaces in, each
/// under the directory named alone on a line two spaces in above it, if one
/// is since the last title.
fn named_in(topic: &str) -> Vec<String> {
    let (mut named, mut under) = (Vec::new(), "");
    for line in topic.lines() {
        let words = line.trim_start();
        match line.len() - words.len() {
            0 => under = "",
            2 if words.ends_with('/') && !words.contains(' ') => under = words,
            4 => named.extend(words.split_whitespace().map(|file| format!("{under}{file}"))),
            _ => {}
        }
    }
    named
}

/// What the binary prints for `args`, which is no JSON object.
fn printed(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(args).output();
    String::from_utf8(out.unwrap().stdout).unwrap()
}

#[test]
fn the_topic_names_every_file_the_example_writes_and_no_other() {
    let topic = printed(&["help", "example"]);
    let hospital = afkconvoy::scaffold::example("hospital").unwrap();
    let mut written: Vec<&str> = hospital.files.iter().map(|(path, _)| *path).collect();
    let mut named = named_in(&topic);
    assert!(named.len() > 40, "{named:?}");
    assert!(named.contains(&"roles/fellow/.claude/hooks/refuse.sh".to_string()), "{named:?}");
    written.sort_unstable();
    named.sort_unstable();
    assert_eq!(named, written);
    // It says the line that writes them, and `scaffold --help` each name
    // `--example` takes.
    assert!(topic.contains("afkconvoy scaffold team hospital --example hospital --repo "));
    let help = printed(&["scaffold", "--help"]);
    for name in afkconvoy::scaffold::example_names() {
        assert!(help.contains(&format!("[possible values: {name}]")), "{help}");
        assert!(help.contains(&format!("--example {name}")), "{help}");
    }
    assert!(help.contains("`scaffold team <dir> --example <name>` writes an example team whole"));
}

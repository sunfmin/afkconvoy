//! Claude Code's native skill discovery, including the host profile when it
//! is above the role or an added project. Opt-in: needs a login and costs a
//! little. Run with `cargo test -p afkconvoy --test cli smoke_claude_skills::
//! -- --ignored --nocapture`. No host configuration is edited.

use std::path::Path;
use std::process::{Command, Stdio};

use afkconvoy::kept::keep_under;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use afkconvoy_core::role::{Facts, On, Start, Told};
use serde_json::Value;

const USER: &str = "convoy-ancestor-user";
const ROLE: &str = "convoy-ancestor-role";
const PROJECT: &str = "convoy-ancestor-project";

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str) {
    write(
        dir,
        &format!(".claude/skills/{name}/SKILL.md"),
        &format!(
            "---\nname: {name}\ndescription: Controlled ancestor discovery sentinel\n---\nReply OK.\n"
        ),
    );
}

fn quoted(text: &str) -> String {
    Line::new().word(text).own()
}

#[test]
#[ignore = "starts real Claude Code: needs a login, costs a little; run by hand"]
fn real_claude_skills_keep_host_ancestors_opt_in() {
    let temp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.afkconvoy-tmp");
    write(&temp, ".gitignore", "*\n");
    let scratch = tempfile::tempdir_in(temp).unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let launch = std::env::var("AFKCONVOY_SMOKE_COMMAND")
        .unwrap_or_else(|_| Harness::ClaudeCode.stock().to_string());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let text = |path: &Path| path.display().to_string();
    for layout in ["role-ancestor", "project-ancestor", "fleet-cache"] {
        let fixture = root.join(layout);
        let home = fixture.join("host");
        let role = match layout {
            "role-ancestor" => home.join("role"),
            "fleet-cache" => home.join(".cache/afkconvoy/copies/acme/dev/abc"),
            _ => fixture.join("role"),
        };
        let project = match layout {
            "project-ancestor" => home.join("project"),
            "fleet-cache" => home.join("Developments/project"),
            _ => fixture.join("project"),
        };
        let team = fixture.join("team");
        skill(&home, USER);
        skill(&role, ROLE);
        skill(&project, PROJECT);
        write(&team, "team.md", "A controlled team.\n");
        for host_user in [false, true] {
            let kept = keep_under(
                &home.join(".cache/afkconvoy"),
                Some(&home),
                Harness::ClaudeCode,
                host_user,
            )
            .unwrap();
            let start = Start {
                role_dir: &text(&role),
                worktree: &text(&project),
                team_dir: &text(&team),
                model: Some("haiku"),
                guard: &format!("{} guard", quoted(env!("CARGO_BIN_EXE_afkconvoy"))),
                kept: &text(&kept),
                host_user,
                dsh_profile: afkconvoy_core::team::DshProfile::Fleet,
                shadowed: &[],
                told: Told::default(),
                facts: Facts {
                    on: On::Stage { issue: 7, flow: "feature", stage: "smoke-stage" },
                    role: "dev",
                    skill: ROLE,
                    writes: false,
                    consult: false,
                    base_branch: "main",
                    escalate_label: "ready-for-human",
                    lock_label: "fleet-lock",
                    role_commit: "abc",
                    team_commit: "def",
                },
            };
            let line = format!(
                "export CLAUDE_SECURESTORAGE_CONFIG_DIR=\"${{CLAUDE_SECURESTORAGE_CONFIG_DIR-${{CLAUDE_CONFIG_DIR-}}}}\" \
                 CLAUDE_CONFIG_DIR={} && {} -p 'Reply OK.' --output-format stream-json \
                 --verbose --no-session-persistence",
                quoted(&text(&home.join(".claude"))),
                Harness::ClaudeCode.in_role(&launch, &start),
            );
            let out =
                Command::new(&shell).args(["-lic", &line]).stdin(Stdio::null()).output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let events: Vec<Value> =
                stdout.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
            let init = events.iter().find(|e| e["type"] == "system" && e["subtype"] == "init");
            assert!(
                out.status.success() && init.is_some(),
                "{layout}, host_user={host_user}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let skills: Vec<&str> = init.unwrap()["skills"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            println!("{layout}, host_user={host_user}: native skills: {skills:?}");
            assert!(skills.contains(&ROLE) && skills.contains(&PROJECT), "{skills:?}");
            assert_eq!(skills.contains(&USER), host_user, "{layout}: {skills:?}");
        }
    }
}

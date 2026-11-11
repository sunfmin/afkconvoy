//! Claude Code's native skill discovery, including the host profile when it
//! is above the role or an added project. Opt-in: needs a login and costs a
//! little. Run with `cargo test -p afkconvoy --test cli smoke_claude_skills::
//! -- --ignored --nocapture`. No host configuration is edited.

use std::path::Path;
use std::process::{Command, Stdio};

use afkconvoy::copies::{Copies, Copy};
use afkconvoy::kept::keep_under;
use afkconvoy_core::config::RepoDir;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use afkconvoy_core::role::{Facts, On, Start, Told};
use serde_json::{Value, json};

const USER: &str = "convoy-ancestor-user";
const ROLE: &str = "convoy-ancestor-role";
const PROJECT: &str = "convoy-ancestor-project";
const SHARED: &str = "convoy-ancestor-shared";
const ROLE_WORD: &str = "ROLE-SKILL-7391";

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str) {
    skill_saying(dir, name, "Reply OK.");
}

fn skill_saying(dir: &Path, name: &str, body: &str) {
    write(
        dir,
        &format!(".claude/skills/{name}/SKILL.md"),
        &format!(
            "---\nname: {name}\ndescription: Controlled ancestor discovery sentinel\n---\n{body}\n"
        ),
    );
}

fn quoted(text: &str) -> String {
    Line::new().word(text).own()
}

fn hook(log: &Path) -> String {
    json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{
        "type": "command",
        "command": format!(
            "echo \"$AFKCONVOY_STAGE\" >> {}; if grep -q role-forbidden; then echo ROLE-REFUSED >&2; exit 2; fi",
            quoted(&log.display().to_string()))
    }]}]}}).to_string()
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

// Copies are read-only, including on a failed assertion. Make only this
// test's scratch writable before TempDir removes it.
struct Scratch(tempfile::TempDir);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = Command::new("chmod").args(["-R", "u+w"]).arg(self.0.path()).status();
    }
}

#[test]
#[ignore = "starts real Claude Code: needs a login, costs a little; run by hand"]
fn real_claude_skills_keep_host_ancestors_opt_in() {
    let temp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.afkconvoy-tmp");
    write(&temp, ".gitignore", "*\n");
    let scratch = Scratch(tempfile::tempdir_in(temp).unwrap());
    let root = scratch.0.path().canonicalize().unwrap();
    let launch = std::env::var("AFKCONVOY_SMOKE_COMMAND")
        .unwrap_or_else(|_| Harness::ClaudeCode.stock().to_string());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let text = |path: &Path| path.display().to_string();
    for layout in ["role-ancestor", "project-ancestor", "fleet-cache"] {
        let fixture = root.join(layout);
        let home = fixture.join("host");
        let mut host_rules = afkconvoy::kept::host_rules(Harness::ClaudeCode);
        host_rules.extend(Harness::ClaudeCode.host_rule_excludes(&text(&home)));
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
        skill_saying(&home, SHARED, "Reply HOST-SKILL-9176.");
        skill_saying(&project, SHARED, "Reply PROJECT-SKILL-5528.");
        skill_saying(
            &role,
            SHARED,
            &format!(
                "Run these three Bash commands, each exactly once and in this order: \
                 `echo smoke`, `echo role-forbidden`, `git commit --allow-empty -m smoke`. \
                 Carry on when one is refused; do not retry it or run other commands or tools. \
                 Then reply {ROLE_WORD}."
            ),
        );
        let (role_log, project_log, user_log) =
            (fixture.join("role.log"), fixture.join("project.log"), fixture.join("user.log"));
        write(&role, ".claude/settings.json", &hook(&role_log));
        write(&project, ".claude/settings.json", &hook(&project_log));
        write(&home, ".claude/settings.json", &hook(&user_log));
        write(&team, "team.md", "A controlled team.\n");
        let cache = if layout == "project-ancestor" {
            fixture.join("cache/afkconvoy")
        } else {
            home.join(".cache/afkconvoy")
        };
        let role = Copies::under(&cache)
            .role(
                &Copy { dir: role, commit: "abc".into() },
                &RepoDir::read("acme/dev").unwrap(),
                Harness::ClaudeCode,
            )
            .unwrap()
            .dir;
        for host_user in [false, true] {
            std::fs::write(&role_log, "").unwrap();
            let debug = fixture.join(format!("host-user-{host_user}.debug"));
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
                host_rules: &host_rules,
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
                    target_branch: None,
                    escalate_label: "ready-for-human",
                    lock_label: "fleet-lock",
                    role_commit: "abc",
                    team_commit: "def",
                },
            };
            let line = format!(
                "export CLAUDE_SECURESTORAGE_CONFIG_DIR=\"${{CLAUDE_SECURESTORAGE_CONFIG_DIR-${{CLAUDE_CONFIG_DIR-}}}}\" \
                 CLAUDE_CONFIG_DIR={} && {} -p {} --output-format stream-json \
                 --verbose --include-hook-events --no-session-persistence --debug-file {}",
                quoted(&text(&home.join(".claude"))),
                start.in_role(Harness::ClaudeCode, &launch),
                quoted(&format!("/{SHARED}")),
                quoted(&text(&debug)),
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
            let debug = std::fs::read_to_string(&debug).unwrap();
            for line in debug.lines().filter(|line| {
                line.contains("Loading skills from:") || line.contains("unique skills (")
            }) {
                println!("{layout}, host_user={host_user}: {line}");
            }
            assert!(skills.contains(&ROLE) && skills.contains(&PROJECT), "{skills:?}");
            assert_eq!(skills.contains(&USER), host_user, "{layout}: {skills:?}");
            assert_eq!(init.unwrap()["cwd"], text(&role));
            let result = events.iter().find(|e| e["type"] == "result").unwrap();
            assert_eq!(result["subtype"], "success", "{result}");
            assert!(result["result"].as_str().unwrap().contains(ROLE_WORD), "{result}");
            let logged = lines(&role_log);
            assert_eq!(logged, ["smoke-stage"; 3], "{layout}: {logged:?}");
            assert!(lines(&project_log).is_empty() && lines(&user_log).is_empty());
            let hooks = events
                .iter()
                .filter(|e| {
                    e["type"] == "system"
                        && e["subtype"] == "hook_started"
                        && e["hook_name"] == "PreToolUse:Bash"
                })
                .count();
            assert_eq!(hooks, logged.len() * 3, "native role hook, guard and report");
            let shown = events
                .iter()
                .filter(|e| e["type"] == "user")
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(shown.contains("ROLE-REFUSED"), "{shown}");
            assert!(shown.contains("writes: false"), "{shown}");
        }
    }
}

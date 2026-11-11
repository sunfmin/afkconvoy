//! Real Claude Code, on the line a bound worker receives. Opt-in: needs the
//! host's login and costs a little. Run after changing that line or upgrading
//! Claude Code:
//!
//!   cargo test -p afkconvoy --test cli smoke:: -- --ignored --nocapture
//!
//! `AFKCONVOY_SMOKE_COMMAND` overrides the stock launch command. A disposable
//! host profile supplies known rules, skills and hooks; the host's real login
//! stays accessible, and none of its configuration is edited. Rule sources
//! come from Claude Code's InstructionsLoaded hook, not a model's file list.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use afkconvoy::kept::keep_under;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use afkconvoy_core::role::{Facts, On, Start, Told};
use serde_json::{Value, json};

const ROLE_WORD: &str = "ROLE-7391";
const PROJECT_WORD: &str = "PROJECT-5528";
const TEAM_WORD: &str = "TEAM-6843";
const USER_WORD: &str = "USER-9176";
const ROLE_SKILL: &str = "convoy-smoke-role-skill";
const PROJECT_SKILL: &str = "convoy-smoke-project-skill";
const USER_SKILL: &str = "convoy-smoke-user-skill";

/// What the role's own hook refuses, and what it says when it does.
const ROLE_REFUSES: &str = "role-forbidden";
const ROLE_SAYS: &str = "ROLE-SAYS-NO-4417";

// None of the words is in the prompt: an answer can know one only from its
// instructions. No reading tools are allowed to fetch an excluded file.
const PROMPT: &str = "Run these three Bash commands, each exactly once and in this order, and \
    carry on when one is refused, without trying it again or another way: `echo smoke`, then \
    `echo role-forbidden`, then `git commit --allow-empty -m smoke`. Use no other tools or \
    commands. Then answer with exactly these four lines and nothing else: \
    ROLE=<the role word your instructions give, or NONE>\n\
    PROJECT=<the project word your instructions give, or NONE>\n\
    TEAM=<the team word your instructions give, or NONE>\n\
    USER=<the user word your instructions give, or NONE>";

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn quoted(text: &str) -> String {
    Line::new().word(text).own()
}

fn skill(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: A throwaway skill of afkconvoy's smoke test\n---\nSay {name}.\n"
    )
}

/// A hook on every Bash call that appends what `says` expands to to `log`,
/// and refuses a call that names `ROLE_REFUSES`.
fn hook(log: &Path, says: &str) -> Value {
    let command = format!(
        "echo {says} >> {}; if grep -q {ROLE_REFUSES}; then echo {ROLE_SAYS} >&2; exit 2; fi",
        quoted(&log.display().to_string())
    );
    json!({"hooks": {"PreToolUse": [{"matcher": "Bash",
        "hooks": [{"type": "command", "command": command}]}]}})
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

fn receipts(log: &Path) -> Vec<Value> {
    lines(log).iter().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
#[ignore = "starts real Claude Code: needs a login, costs a little; run by hand"]
fn real_claude_code_started_in_a_role_keeps_host_rules_opt_in_and_enforces_both_guards() {
    // The fleet may give its worker a long TMPDIR inside the checkout. Keep
    // all fixtures there, and canonicalize paths as Claude Code reports them.
    let scratch = tempfile::TempDir::new().unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let home = root.join("host");
    let ancestor = root.join("ancestor");
    let (role, project, team) =
        (ancestor.join("role"), ancestor.join("project"), ancestor.join("team"));
    let config = home.join(".claude");
    let user_log = root.join("user-hook.log");
    write(&config, "CLAUDE.md", &format!("The user word is {USER_WORD}.\n"));
    // Exercise ancestor rule discovery independently of the user-profile
    // skill loader: the latter is governed by --setting-sources.
    write(&ancestor, ".claude/CLAUDE.md", "A host rule found through project ancestry.\n");
    write(
        &ancestor,
        ".claude/rules/smoke.md",
        "Another host rule found through project ancestry.\n",
    );
    write(&config, &format!("skills/{USER_SKILL}/SKILL.md"), &skill(USER_SKILL));
    write(&config, "settings.json", &hook(&user_log, "user").to_string());
    write(&role, "CLAUDE.md", &format!("The role word is {ROLE_WORD}.\n"));
    write(&role, &format!(".claude/skills/{ROLE_SKILL}/SKILL.md"), &skill(ROLE_SKILL));
    write(&project, "CLAUDE.md", &format!("The project word is {PROJECT_WORD}.\n"));
    write(&project, &format!(".claude/skills/{PROJECT_SKILL}/SKILL.md"), &skill(PROJECT_SKILL));
    write(&team, "CLAUDE.md", &format!("The team word is {TEAM_WORD}.\n"));
    write(&team, "team.md", "a team\n");

    let launch = std::env::var("AFKCONVOY_SMOKE_COMMAND")
        .unwrap_or_else(|_| Harness::ClaudeCode.stock().to_string());
    let mut host_rules = afkconvoy::kept::host_rules(Harness::ClaudeCode);
    host_rules.extend(Harness::ClaudeCode.host_rule_excludes(&home.display().to_string()));
    host_rules.extend(Harness::ClaudeCode.host_rule_excludes(&ancestor.display().to_string()));
    let text = |path: &Path| path.display().to_string();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    for host_user in [false, true] {
        let run = root.join(format!("host-user-{host_user}"));
        let (role_log, project_log, sources_log) =
            (run.join("role-hook.log"), run.join("project-hook.log"), run.join("sources.jsonl"));
        std::fs::create_dir_all(&run).unwrap();
        let mut settings = hook(&role_log, "\"$AFKCONVOY_STAGE\"");
        // One append per event, including its newline: concurrent loads
        // cannot splice two JSON objects together.
        settings["hooks"]["InstructionsLoaded"] = json!([{"hooks": [{
            "type": "command", "command": format!(
                "input=$(cat); printf '%s\\n' \"$input\" >> {}", quoted(&text(&sources_log)))
        }]}]);
        write(&role, ".claude/settings.json", &settings.to_string());
        write(&project, ".claude/settings.json", &hook(&project_log, "project").to_string());
        let kept =
            keep_under(&run.join("cache"), Some(&home), Harness::ClaudeCode, host_user).unwrap();
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
                skill: ROLE_SKILL,
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
        // Relocate only the test's host profile, retaining the original
        // credential store (including macOS's directory-keyed Keychain).
        // Do this after the login shell's startup files have run.
        let line = format!(
            "export CLAUDE_SECURESTORAGE_CONFIG_DIR=\"${{CLAUDE_SECURESTORAGE_CONFIG_DIR-${{CLAUDE_CONFIG_DIR-}}}}\" \
             CLAUDE_CONFIG_DIR={} && {} -p {} \
             --output-format stream-json --verbose --include-hook-events --no-session-persistence",
            quoted(&text(&config)),
            start.in_role(Harness::ClaudeCode, &launch),
            quoted(PROMPT)
        );
        let out = Command::new(&shell).args(["-lic", &line]).stdin(Stdio::null()).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let events: Vec<Value> =
            stdout.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
        let of = |kind: &str, subtype: &str| -> Vec<&Value> {
            events
                .iter()
                .filter(|event| event["type"] == kind && event["subtype"] == subtype)
                .collect()
        };
        let (init, result) = (of("system", "init"), of("result", "success"));
        assert!(
            out.status.success() && init.len() == 1 && result.len() == 1,
            "host_user={host_user}: Claude Code did not answer: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let init = init[0];
        let said = result[0]["result"].as_str().unwrap_or_default();
        let skills: Vec<&str> =
            init["skills"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(init["cwd"], text(&role));
        assert!(init["model"].as_str().unwrap().contains("haiku"), "{}", init["model"]);
        assert!(init["memory_paths"].is_null(), "{}", init["memory_paths"]);

        let sources: Vec<Value> = receipts(&sources_log)
            .iter()
            .map(|event| {
                json!({
                    "file_path": event["file_path"], "memory_type": event["memory_type"],
                    "load_reason": event["load_reason"]
                })
            })
            .collect();
        println!("host_user={host_user}: rule sources: {sources:?}");
        let loaded = |file: PathBuf| sources.iter().any(|event| event["file_path"] == text(&file));
        for (name, word, file) in [
            ("ROLE", ROLE_WORD, role.join("CLAUDE.md")),
            ("PROJECT", PROJECT_WORD, project.join("CLAUDE.md")),
            ("TEAM", TEAM_WORD, team.join("CLAUDE.md")),
        ] {
            assert!(loaded(file), "{name} rules were not loaded: {sources:?}");
            assert!(said.contains(&format!("{name}={word}")), "{said}");
        }
        assert!(skills.contains(&ROLE_SKILL) && skills.contains(&PROJECT_SKILL), "{skills:?}");
        // A native source receipt and a word absent from the prompt agree.
        // The positive run proves the sentinel is discoverable, not vacuous.
        let user_loaded =
            loaded(config.join("CLAUDE.md")) || loaded(kept.join(".claude/CLAUDE.md"));
        assert_eq!(user_loaded, host_user, "host_user={host_user}: {sources:?}");
        for path in [".claude/CLAUDE.md", ".claude/rules/smoke.md"] {
            assert_eq!(loaded(ancestor.join(path)), host_user, "{sources:?}");
        }
        if !host_user {
            for path in &host_rules {
                let prefix = path.strip_suffix("**").unwrap_or(path);
                assert!(
                    !sources.iter().any(|event| event["file_path"]
                        .as_str()
                        .is_some_and(|file| file.starts_with(prefix))),
                    "host rule was loaded: {sources:?}"
                );
            }
        }
        assert_eq!(skills.contains(&USER_SKILL), host_user, "{skills:?}");
        let user_word = if host_user { USER_WORD } else { "NONE" };
        assert!(said.contains(&format!("USER={user_word}")), "{said}");

        // Every Bash call runs exactly the role hook, fleet guard and Orca
        // report. Hooks from the added project and the host never run.
        let logged = lines(&role_log);
        assert_eq!(logged, ["smoke-stage"; 3]);
        assert_eq!(lines(&project_log), [] as [String; 0]);
        assert_eq!(lines(&user_log), [] as [String; 0]);
        let fired: Vec<&str> =
            of("system", "hook_started").iter().filter_map(|e| e["hook_name"].as_str()).collect();
        let on_bash = fired.iter().filter(|name| **name == "PreToolUse:Bash").count();
        assert_eq!(on_bash, logged.len() * 3, "the role, guard and report: {fired:?}");

        let shown: Vec<String> = events
            .iter()
            .filter(|event| event["type"] == "user")
            .flat_map(|event| event["message"]["content"].as_array().cloned().unwrap_or_default())
            .filter(|part| part["type"] == "tool_result")
            .map(|part| part["content"].to_string())
            .collect();
        let shown_once = |what: &str| shown.iter().filter(|text| text.contains(what)).count() == 1;
        assert!(
            shown.iter().any(|text| text == r#""smoke""#),
            "the allowed Bash command did not run: {shown:?}"
        );
        assert!(shown_once(ROLE_SAYS), "the role's hook refused nothing: {shown:?}");
        assert!(
            shown_once(
                "the stage `smoke-stage` is marked `writes: false`: `git commit` is refused"
            ),
            "the guard refused nothing: {shown:?}"
        );
        println!(
            "host_user={host_user}: {said}; three Bash calls each ran the role hook, fleet guard \
            and Orca report; project and host hooks stayed off; role and guard refusals reached Claude"
        );
    }
}

/// A real interactive start, unlike the print-mode smoke above: print mode
/// skips workspace trust itself and would hide a broken interactive launch.
/// Each directory is a fresh Git root so trust in an ancestor cannot mask it.
#[test]
#[ignore = "starts real Claude Code in Orca: needs both running and a login; costs a little"]
fn real_interactive_claude_takes_its_brief_without_persisting_workspace_trust() {
    use afkconvoy_core::line::Line;

    fn orca(args: &[&str]) -> Value {
        let out = Command::new("orca").args(args).arg("--json").output().unwrap();
        let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(answer["ok"], true, "{args:?}: {answer}");
        answer["result"].clone()
    }
    struct Terminal(String);
    impl Drop for Terminal {
        fn drop(&mut self) {
            let _ = Command::new("orca")
                .args(["terminal", "close", "--terminal", &self.0, "--json"])
                .output();
        }
    }
    fn persisted_trust(path: &Path) -> bool {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let config = match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir).join(".claude.json"),
            None => home.join(".claude.json"),
        };
        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(config).unwrap_or_else(|_| "{}".into()))
                .unwrap();
        config["projects"][path.to_str().unwrap()]["hasTrustDialogAccepted"] == true
    }

    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let tmp = checkout.join(".afkconvoy-tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let scratch = tempfile::Builder::new().prefix("claude-trust-").tempdir_in(tmp).unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let (role, project, team, kept) =
        (root.join("role"), root.join("project"), root.join("team"), root.join("kept"));
    for dir in [&role, &project, &team, &kept] {
        std::fs::create_dir(dir).unwrap();
    }
    for dir in [&role, &project] {
        assert!(
            Command::new("git").args(["init", "-q"]).current_dir(dir).status().unwrap().success()
        );
        assert!(!persisted_trust(dir));
    }
    let text = |path: &Path| path.display().to_string();
    let launch = Harness::ClaudeCode.stock();
    for context in ["unbound", "role", "role-host-user"] {
        let line = if context == "unbound" {
            let trusted = Harness::ClaudeCode.in_worktree(launch, &text(&project), "");
            Line::new().fixed("cd ").word(&text(&project)).fixed(" && ").launch(&trusted).done()
        } else {
            let start = Start {
                role_dir: &text(&role),
                worktree: &text(&project),
                team_dir: &text(&team),
                kept: &text(&kept),
                model: Some("claude-opus-5-5"),
                guard: &format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy")),
                host_user: context == "role-host-user",
                dsh_profile: afkconvoy_core::team::DshProfile::Fleet,
                host_rules: &afkconvoy::kept::host_rules(Harness::ClaudeCode),
                shadowed: &[],
                told: Told::default(),
                facts: Facts {
                    on: On::Stage { issue: 7, flow: "feature", stage: "smoke-stage" },
                    role: "dev",
                    skill: ROLE_SKILL,
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
            let line = start.in_role(Harness::ClaudeCode, launch);
            assert_eq!(line.matches("--settings ").count(), 2);
            line
        };
        let selector = format!("path:{}", checkout.display());
        let made = orca(&["terminal", "create", "--worktree", &selector, "--command", &line]);
        let terminal = Terminal(made["terminal"]["handle"].as_str().unwrap().into());
        let waited = orca(&[
            "terminal",
            "wait",
            "--terminal",
            &terminal.0,
            "--for",
            "tui-idle",
            "--timeout-ms",
            "30000",
        ]);
        let screen =
            orca(&["terminal", "read", "--terminal", &terminal.0, "--screen", "--limit", "200"]);
        println!("{context}: wait={waited}; screen={screen}");
        assert_eq!(waited["wait"]["satisfied"], true, "{waited}; {screen}");
        assert!(waited["wait"]["blockedReason"].is_null(), "{waited}");
        assert_eq!(screen["terminal"]["source"], "screen", "{screen}");
        assert!(!screen.to_string().contains("Yes, I trust this folder"), "{screen}");
        let sent = orca(&[
            "terminal",
            "send",
            "--terminal",
            &terminal.0,
            "--text",
            "Reply with exactly TRUST-SMOKE-DONE. Use no tools.",
            "--enter",
            "--wait-submit",
            "30",
        ]);
        assert!(
            sent["send"]["prompt"]["stages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stage| stage == "turn_started"),
            "{sent}"
        );
        println!("{context}: receipt={sent}");
        drop(terminal);
        assert!(!persisted_trust(&role));
        assert!(!persisted_trust(&project));
    }
}

/// Opt-in live acceptance check: the named issue must already be claimed
/// on this host in this worktree. Only its new read-only comment is delivered;
/// every other issue is left alone.
#[test]
#[ignore = "tells a real claimed worker through Orca; posts one issue comment; AFKCONVOY_LIVE_ISSUE=N"]
fn a_real_claimed_worker_receives_a_later_comment_in_its_session() {
    use afkconvoy::fleet::Fleet;
    use afkconvoy::issue_updates::reconcile;
    use serde_json::json;
    use std::time::{Duration, Instant};

    let issue: u64 =
        std::env::var("AFKCONVOY_LIVE_ISSUE").expect("name the claimed issue").parse().unwrap();
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let repo = afkconvoy::git::Repo::new(checkout.clone());
    let fleet = Fleet::open(
        &repo,
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
    )
    .unwrap();
    let host = afkconvoy::host_settings::host_name();
    assert_eq!(fleet.scan.claim_of(issue).unwrap().owner.as_ref().unwrap().host, host);
    let text = format!(
        "Read-only delivery acceptance check {}: reply RECEIVED. This comment requires no code change.",
        uuid::Uuid::new_v4()
    );
    let existing = std::env::var("AFKCONVOY_LIVE_COMMENT_URL").ok();
    if existing.is_none() {
        fleet.gh.comment(issue, &text).unwrap();
    }
    let comment = fleet
        .gh
        .comments(issue)
        .unwrap()
        .into_iter()
        .find(|comment| existing.as_ref().map_or(comment.body == text, |url| &comment.url == url))
        .unwrap();
    let worker =
        afkconvoy::orca::Worktree::of_issue(&afkconvoy::orca::Source::of(&repo).unwrap(), issue)
            .unwrap()
            .unwrap();
    let harness = worker.recorded_harness().unwrap().unwrap();
    let started = worker.handed(None).unwrap().unwrap();
    let began = Instant::now();
    loop {
        let mut open = fleet.gh.open().unwrap();
        open.issues.rows.retain(|row| row.number == issue);
        let steps = reconcile(&fleet, &host, &open.issues);
        if afkconvoy::session::holds_note(&checkout, harness, started, &comment.url).unwrap() {
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(90), "{steps:?}");
        std::thread::sleep(Duration::from_millis(250));
    }
    let entry_at = afkconvoy::session::note_at(&checkout, harness, started, &comment.url)
        .unwrap()
        .expect("the real session's user entry");
    let evidence = json!({"issue":issue,"comment":comment.url,"comment_at":comment.at,
        "session_entry_at":entry_at,"seconds":entry_at.saturating_sub(comment.at.unwrap()),
        "terminal":worker.terminal().unwrap(),"harness":harness});
    std::fs::create_dir_all(checkout.join(".afkconvoy-tmp")).unwrap();
    std::fs::write(
        checkout.join(".afkconvoy-tmp/issue-update-live-evidence.json"),
        evidence.to_string(),
    )
    .unwrap();
    println!("{evidence}");
}

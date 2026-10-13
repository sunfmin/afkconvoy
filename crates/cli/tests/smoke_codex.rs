//! The one test against real Codex. Every other test starts a Codex worker
//! in a stand-in orca and runs its line under a stand-in Codex, which proves
//! the fleet builds the right line — not that Codex answers it as assumed
//! (docs/adr/0013). This one runs that line for real, in a throwaway role
//! with a throwaway project as its worktree, under a throwaway home that
//! holds rules of a host user's a worker must not load, and reads which
//! rules Codex would show the model.
//!
//! It is off by default and outside the gate: it needs Codex installed. It
//! needs no login and costs nothing: Codex is asked what it would show the
//! model (`codex debug prompt-input`), and calls none. Run it by hand when
//! what the fleet adds to the worker launch command changes, or Codex is
//! upgraded:
//!
//!   cargo test -p afkconvoy --test smoke_codex -- --ignored --nocapture
//!
//! `AFKCONVOY_SMOKE_CODEX_COMMAND` is the worker launch command to try, when
//! it is not the stock one.

use std::path::Path;
use std::process::{Command, Stdio};

use afkconvoy::kept::keep_under;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::role::{Facts, On, Rules, Start, Told};
use serde_json::Value;

const ROLE_WORD: &str = "ROLE-7391";
const ABOVE_WORD: &str = "ABOVE-6102";
const PROJECT_WORD: &str = "PROJECT-5528";
const OTHER_WORD: &str = "OTHER-3310";
const TEAM_WORD: &str = "TEAM-8846";
const USER_WORD: &str = "USER-9920";

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Every text of the messages `shown` holds for `role`, one after another.
fn said(shown: &Value, role: &str) -> String {
    let messages = shown.as_array().unwrap().iter().filter(|message| message["role"] == role);
    let texts = messages.flat_map(|message| message["content"].as_array().cloned());
    texts.flatten().filter_map(|part| part["text"].as_str().map(str::to_string)).collect()
}

#[test]
#[ignore = "runs real Codex: cargo test -p afkconvoy --test smoke_codex -- --ignored"]
fn a_real_codex_worker_is_shown_its_role_s_rules_the_repo_s_own_and_the_team_rules() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (above, project, team) = (root.join("above"), root.join("project"), root.join("team"));
    let (role, home, cache) = (above.join("role"), root.join("home"), root.join("cache"));

    // A role in a repo whose root holds rules too, as a role opened by hand
    // in its subdirectory would be: a worker is given the role's alone.
    write(&role, "CLAUDE.md", &format!("# Role rules\n\nThe role word is {ROLE_WORD}.\n"));
    write(&above, "CLAUDE.md", &format!("The word above is {ABOVE_WORD}.\n"));
    write(&above, ".git/HEAD", "ref: refs/heads/main\n");
    // The repo's own rules are in the first file a worktree holds: the other
    // is what the fleet did not read.
    let (own, other) = (
        format!("# Project rules\n\nThe \"project\" word is {PROJECT_WORD}.\n"),
        format!("The other word is {OTHER_WORD}.\n"),
    );
    write(&project, "AGENTS.md", &own);
    write(&project, "CLAUDE.md", &other);
    let team_rules = format!("The team's word is {TEAM_WORD}.\n");
    write(&team, "CLAUDE.md", &team_rules);
    // The host user's own rules, which Codex loads from its user directory.
    write(&home, ".codex/AGENTS.md", &format!("The user word is {USER_WORD}.\n"));

    let launch = std::env::var("AFKCONVOY_SMOKE_CODEX_COMMAND")
        .unwrap_or_else(|_| Harness::Codex.stock().to_string());
    let text = |path: &Path| path.display().to_string();
    let kept = keep_under(&cache, Some(&home), Harness::Codex).unwrap();
    let names = Harness::Codex.told();
    let held = names.iter().map(|name| project.join(name)).find(|file| file.is_file()).unwrap();
    assert_eq!(held, project.join("AGENTS.md"));
    let start = Start {
        role_dir: &text(&role),
        worktree: &text(&project),
        team_dir: &text(&team),
        model: None,
        guard: &format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy")),
        kept: &text(&kept),
        told: Told {
            repo: Some(Rules { file: &text(&held), text: &own }),
            team: Some(Rules { file: &text(&team.join("CLAUDE.md")), text: &team_rules }),
        },
        facts: Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "smoke-stage" },
            role: "dev",
            skill: "implement",
            writes: false,
            consult: false,
            base_branch: "main",
            escalate_label: "ready-for-human",
            lock_label: "fleet-lock",
            role_commit: "abc",
            team_commit: "def",
        },
    };
    // The very line a bound repo's dispatch hands orca, and what has Codex
    // say what it would show the model instead of starting a session.
    let line = format!("{} debug prompt-input go", Harness::Codex.in_role(&launch, &start));
    let out = Command::new("/bin/sh")
        .args(["-c", &line])
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{printed}\n{}", String::from_utf8_lossy(&out.stderr));
    let shown: Value = serde_json::from_str(&printed).unwrap_or_else(|e| panic!("{e}: {printed}"));
    let all = shown.to_string();

    // The role's rules are loaded by Codex itself, from the file every
    // harness loads them from — and nothing from the directory above.
    assert!(said(&shown, "user").contains(ROLE_WORD), "the role's rules were not loaded: {all}");
    assert!(!all.contains(ABOVE_WORD), "rules above the role were loaded: {all}");
    // The repo's own rules and the team rules are told, each as what it is.
    let told = said(&shown, "developer");
    let (repo_at, team_at) = (
        told.find(&format!("# The repo's own rules: {}", held.display())),
        told.find(&format!("# The team rules: {}", team.join("CLAUDE.md").display())),
    );
    assert!(repo_at.is_some() && repo_at < team_at, "{told}");
    assert!(told.contains(&format!("The \"project\" word is {PROJECT_WORD}.")), "{told}");
    assert!(told.contains(TEAM_WORD), "{told}");
    // Each is shown once: Codex loads none of them from an added directory.
    for word in [ROLE_WORD, PROJECT_WORD, TEAM_WORD] {
        assert_eq!(all.matches(word).count(), 1, "{word}: {all}");
    }
    assert!(!all.contains(OTHER_WORD), "the worktree's other rules file was loaded: {all}");
    // Nothing of the user's: the fleet's own user directory holds no rules.
    assert!(!all.contains(USER_WORD), "the user's rules were loaded: {all}");
    assert!(!kept.join("AGENTS.md").exists());
}

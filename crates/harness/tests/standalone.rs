//! The harness crate as another program uses it (docs/adr/0058): from
//! outside, through its public API alone, with no fleet beside it. A start
//! line is built and a recorded session's turn and usage are read for Claude
//! Code and Codex, from nothing but what a caller says.

use afkconvoy_harness::tokens::Counts;
use afkconvoy_harness::turn::Turn;
use afkconvoy_harness::{Answer, DshProfile, Harness, Start, Told};

fn start(model: Option<&str>) -> Start<'_> {
    Start {
        dir: "/agents/reviewer",
        worktree: "/src/project",
        added: "/agents/shared",
        model,
        hook: "/bin/check-call",
        kept: "/cache/kept",
        host_user: false,
        host_rules: &[],
        dsh_profile: DshProfile::default(),
        shadowed: &[],
        told: Told::default(),
        env: vec![("GUILD_TASK", "it's 7".to_string())],
    }
}

#[test]
fn a_start_line_is_built_from_a_neutral_start() {
    for harness in [Harness::ClaudeCode, Harness::Codex] {
        let line = harness.in_role(harness.program(), &start(Some("a-model")));
        let key = harness.key();
        assert!(line.starts_with("cd '/agents/reviewer' && export "), "{key}: {line}");
        // The caller's variables are its own, each value one shell word.
        assert!(line.contains(" GUILD_TASK='it'\\''s 7' && "), "{key}: {line}");
        assert!(line.contains(&format!(" && {} ", harness.program())), "{key}: {line}");
        assert!(line.contains("'/src/project'"), "{key}: {line}");
        assert!(line.contains("a-model"), "{key}: {line}");
        // The hook runs before each tool call, however the harness wires it.
        assert!(line.contains("/bin/check-call"), "{key}: {line}");
        // The crate adds no variable of a fleet's.
        assert!(!line.contains("AFKCONVOY_ISSUE"), "{key}: {line}");
        assert!(!harness.in_role("x", &start(None)).contains("a-model"), "{key}");
    }
}

#[test]
fn a_recorded_claude_code_session_says_its_turn_and_usage() {
    let text = r#"{"type":"user","timestamp":"1970-01-01T00:01:40Z","cwd":"/src/project","isSidechain":false,"message":{"role":"user","content":"go"}}
{"type":"assistant","timestamp":"1970-01-01T00:01:50Z","cwd":"/src/project","isSidechain":false,"message":{"id":"m1","role":"assistant","model":"claude-x","stop_reason":"end_turn","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":3,"output_tokens":5}}}"#;
    let turn = Harness::ClaudeCode.recorded_turn("/src/project", text, None, None);
    assert_eq!(turn, Some(Turn::Closed { since: 110, started: Some(100) }));
    let tokens = Harness::ClaudeCode.tokens(text, None).unwrap();
    assert_eq!(
        tokens["claude-x"],
        Counts { input: 10, cache_read: 20, cache_write: 3, output: 5, reasoning: None }
    );
}

#[test]
fn a_recorded_codex_session_says_its_usage() {
    let text = r#"{"timestamp":90,"type":"turn_context","payload":{"model":"codex-x"}}
{"timestamp":95,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":30,"output_tokens":10,"reasoning_output_tokens":4}}}}"#;
    let tokens = Harness::Codex.tokens(text, None).unwrap();
    assert_eq!(
        tokens["codex-x"],
        Counts { input: 70, cache_read: 30, cache_write: 0, output: 10, reasoning: Some(4) }
    );
}

const OTHERS: [Harness; 4] = [Harness::Pi, Harness::Dsh, Harness::Kimi, Harness::Grok];

#[test]
fn a_claude_code_hook_stops_its_session_with_the_answer_its_hooks_documentation_gives() {
    let Answer { printed, code } = Harness::ClaudeCode.stop_answer("Stopped: \"paused\"").unwrap();
    // https://code.claude.com/docs/en/hooks, "Common JSON fields", read from
    // a hook that exits 0.
    assert_eq!(code, 0);
    assert_eq!(printed, r#"{"continue":false,"stopReason":"Stopped: \"paused\""}"#);
    let parsed: serde_json::Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(parsed, serde_json::json!({"continue": false, "stopReason": "Stopped: \"paused\""}));
    assert_ne!(code, Harness::ClaudeCode.refused(), "a stop is no refusal of the one call");
    assert!(Harness::ClaudeCode.stop_reliable());
}

#[test]
fn a_codex_hook_says_the_same_and_is_not_relied_on() {
    let Answer { printed, code } = Harness::Codex.stop_answer("paused").unwrap();
    assert_eq!((printed.as_str(), code), (r#"{"continue":false,"stopReason":"paused"}"#, 0));
    assert!(!Harness::Codex.stop_reliable());
}

#[test]
fn a_harness_with_no_known_stop_answer_has_none_and_is_not_relied_on() {
    for harness in OTHERS {
        assert_eq!(harness.stop_answer("paused"), None, "{}", harness.key());
        assert!(!harness.stop_reliable(), "{}", harness.key());
    }
}

#[test]
fn a_kept_claude_code_session_is_resumed_in_its_role_with_a_new_instruction() {
    let start = start(Some("a-model"));
    let started = Harness::ClaudeCode.in_role("claude", &start);
    let resumed = Harness::ClaudeCode.resumed("claude", &start, Some("5e1d-ab"), "go on").unwrap();
    assert_eq!(resumed, format!("{started} --resume '5e1d-ab' 'go on'"));
    let latest = Harness::ClaudeCode.resumed("claude", &start, None, "it's next").unwrap();
    assert_eq!(latest, format!("{started} --continue 'it'\\''s next'"));
    // Its id is what each entry of its session file carries.
    let text = r#"{"type":"user","sessionId":"5e1d-ab","cwd":"/agents/reviewer"}"#;
    assert_eq!(Harness::ClaudeCode.session_id(text).as_deref(), Some("5e1d-ab"));
    assert_eq!(Harness::ClaudeCode.session_id("{}"), None);
}

#[test]
fn a_kept_codex_session_is_resumed_in_its_role_with_a_new_instruction() {
    let start = start(Some("a-model"));
    let started = Harness::Codex.in_role("codex", &start);
    let resumed = Harness::Codex.resumed("codex", &start, Some("0199-c0"), "go on").unwrap();
    assert_eq!(resumed, format!("{started} resume '0199-c0' 'go on'"));
    let latest = Harness::Codex.resumed("codex", &start, None, "go on").unwrap();
    assert_eq!(latest, format!("{started} resume --last 'go on'"));
    // Its id is its thread's, which its session_meta gives.
    let text = r#"{"timestamp":"t","type":"session_meta","payload":{"id":"0199-c0","cwd":"/src/project"}}
{"timestamp":"t","type":"turn_context","payload":{"model":"codex-x"}}"#;
    assert_eq!(Harness::Codex.session_id(text).as_deref(), Some("0199-c0"));
    assert_eq!(Harness::Codex.session_id(r#"{"type":"turn_context","payload":{"id":"x"}}"#), None);
}

#[test]
fn a_harness_that_cannot_continue_a_session_has_no_resume_line() {
    for harness in OTHERS {
        assert_eq!(
            harness.resumed("x", &start(None), Some("s"), "go on"),
            None,
            "{}",
            harness.key()
        );
        assert_eq!(harness.session_id(r#"{"sessionId":"s"}"#), None, "{}", harness.key());
    }
}

/// What the crate depends on: Flux's attributes, serde and TOML — never
/// afkconvoy-core, whose fleet concepts its API is to stay free of.
#[test]
fn the_crate_depends_on_nothing_of_the_fleet() {
    let manifest: toml::Table =
        include_str!("../Cargo.toml").parse().expect("the crate's manifest is TOML");
    let mut names: Vec<&str> =
        manifest["dependencies"].as_table().unwrap().keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["flux-alloc", "flux-core", "flux-rs", "serde", "serde_json", "toml"]);
}

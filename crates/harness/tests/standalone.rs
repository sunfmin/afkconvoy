//! The harness crate as another program uses it (docs/adr/0058): from
//! outside, through its public API alone, with no fleet beside it. A start
//! line is built and a recorded session's turn and usage are read for Claude
//! Code and Codex, from nothing but what a caller says.

use afkconvoy_harness::tokens::Counts;
use afkconvoy_harness::turn::Turn;
use afkconvoy_harness::{DshProfile, Harness, Start, Told};

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

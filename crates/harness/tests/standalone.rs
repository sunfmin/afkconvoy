//! The harness crate as another program uses it (docs/adr/0058): from
//! outside, through its public API alone, with no fleet beside it. A start
//! line is built and a recorded session's turn and usage are read for Claude
//! Code and Codex, from nothing but what a caller says; each engine's usage
//! windows and structured limits are read from its recorded samples
//! (`recorded/`, whose README says where each came from).

use afkconvoy_harness::brief::brief_pointer;
use afkconvoy_harness::status::{CallEnd, Reading, Report};
use afkconvoy_harness::tokens::Counts;
use afkconvoy_harness::turn::Turn;
use afkconvoy_harness::window::{Limit, Limited, Window};
use afkconvoy_harness::{Answer, BriefAtStart, DshProfile, Harness, Start, Told};

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

fn records(text: &str) -> Vec<serde_json::Value> {
    text.lines().map(|line| serde_json::from_str(line).expect("a recorded line is JSON")).collect()
}

/// `windows` as `(name, used percent, minutes, resets at)`, the percent
/// rounded to a hundredth: an engine that says a ratio is read times 100.
fn said(windows: &[Window]) -> Vec<(&str, f64, Option<u64>, Option<u64>)> {
    windows
        .iter()
        .map(|w| {
            (w.name.as_str(), (w.used_percent * 100.0).round() / 100.0, w.minutes, w.resets_at)
        })
        .collect()
}

#[test]
fn claude_code_s_status_line_input_says_its_windows() {
    let input: serde_json::Value =
        serde_json::from_str(include_str!("recorded/claude-code-status-line.json")).unwrap();
    assert_eq!(
        said(&Harness::ClaudeCode.windows(&input)),
        [("five_hour", 6.0, None, Some(1794450000)), ("seven_day", 19.0, None, Some(1794740400))]
    );
    assert_eq!(Harness::ClaudeCode.limit_event(&input), None);
    // A host installs the command that is handed that input where Claude
    // Code reads its user's settings.
    let line = Harness::ClaudeCode.status_line().unwrap();
    assert_eq!((line.settings, line.key), (".claude/settings.json", "statusLine"));
    assert_eq!(line.entry("guild read")["command"], "guild read");
    for harness in Harness::ALL {
        if harness != Harness::ClaudeCode {
            assert_eq!(harness.status_line(), None, "{}", harness.key());
        }
    }
}

#[test]
fn claude_code_s_stream_says_its_windows_and_a_rejected_request() {
    let lines = records(include_str!("recorded/claude-code-stream.jsonl"));
    let harness = Harness::ClaudeCode;
    assert_eq!(
        said(&harness.windows(&lines[0])),
        [("five_hour", 6.0, None, Some(1794450000)), ("seven_day", 19.0, None, Some(1794740400))]
    );
    assert_eq!(harness.limit_event(&lines[0]), None, "an allowed request met no limit");
    let weekly = Limit {
        kind: Limited::Usage,
        window: Some("seven_day".into()),
        resets_at: Some(1792494000),
    };
    assert_eq!(harness.limit_event(&lines[1]), Some(weekly));
    assert!(harness.windows(&lines[1]).is_empty(), "a rejection that says no use says no window");
    let unstated = Limit { kind: Limited::Usage, window: None, resets_at: None };
    assert_eq!(harness.limit_event(&lines[2]), Some(unstated.clone()));
    assert_eq!(harness.limit_event(&lines[3]), Some(unstated));
}

#[test]
fn a_claude_code_session_says_a_usage_limit_apart_from_a_throttle() {
    let lines = records(include_str!("recorded/claude-code-session-rate-limit.jsonl"));
    let harness = Harness::ClaudeCode;
    assert_eq!(
        harness.limit_event(&lines[0]),
        Some(Limit {
            kind: Limited::Usage,
            window: Some("seven_day".into()),
            resets_at: Some(1792494000)
        })
    );
    assert_eq!(
        harness.limit_event(&lines[1]),
        Some(Limit { kind: Limited::Throttle, window: None, resets_at: None })
    );
    // Its usage is read as it always was: a synthetic message counts nothing.
    let text = include_str!("recorded/claude-code-session-rate-limit.jsonl");
    assert_eq!(harness.tokens(text, None).unwrap()["<synthetic>"], Counts::default());
}

#[test]
fn a_codex_session_s_token_count_says_its_windows_and_the_limit_reached() {
    let text = include_str!("recorded/codex-session.jsonl");
    let lines = records(text);
    let harness = Harness::Codex;
    assert!(harness.windows(&lines[0]).is_empty());
    assert_eq!(
        said(&harness.windows(&lines[1])),
        [
            ("primary", 81.0, Some(300), Some(1793390439)),
            ("secondary", 53.0, Some(10080), Some(1793800000))
        ]
    );
    assert_eq!(harness.limit_event(&lines[1]), None, "no limit is reached at 81%");
    // An older Codex counted the reset from the line's time.
    let written = 1792483708; // 2026-10-20T08:08:28Z
    assert_eq!(
        said(&harness.windows(&lines[2])),
        [
            ("primary", 100.0, Some(300), Some(written + 4454)),
            ("secondary", 62.0, Some(10080), Some(written + 60713))
        ]
    );
    assert_eq!(
        harness.limit_event(&lines[3]),
        Some(Limit {
            kind: Limited::Usage,
            window: Some("primary".into()),
            resets_at: Some(1793390439)
        })
    );
    // The same lines' usage is read as it always was.
    let tokens = harness.tokens(text, None).unwrap();
    assert_eq!(tokens["gpt-5.5"].input, 25301 - 11008);
}

#[test]
fn kimi_code_s_usage_answer_says_its_windows_and_its_session_none() {
    let answer: serde_json::Value =
        serde_json::from_str(include_str!("recorded/kimi-usages.json")).unwrap();
    assert_eq!(
        said(&Harness::Kimi.windows(&answer)),
        [("limit_5h", 25.0, None, Some(1794495600)), ("limit_7d", 50.0, None, Some(1794787200))]
    );
    assert_eq!(Harness::Kimi.limit_event(&answer), None);
    // A harness whose engine reports no window gives none: its caller predicts.
    for harness in [Harness::Pi, Harness::Dsh, Harness::Grok] {
        assert!(harness.windows(&answer).is_empty(), "{}", harness.key());
    }
}

/// A session nobody types at, as Claude Code's started with its brief: the
/// start line ends its flags and gives the brief pointer as its first prompt,
/// and the session that prompt starts is the one found handed that brief.
#[test]
fn claude_code_is_handed_its_brief_on_its_start_line() {
    let brief = "/git/worktrees/it's 7/brief.md";
    let line = Harness::ClaudeCode.in_role("claude", &start(Some("a-model")));
    let BriefAtStart::Started(started) = Harness::ClaudeCode.brief_at_start(&line, brief) else {
        panic!("Claude Code takes its brief on its start line");
    };
    let pointer = brief_pointer(brief);
    assert_eq!(started, format!("{line} -- '{}'", pointer.replace('\'', "'\\''")));
    let session = format!(
        r#"{{"type":"user","cwd":"/src/project","isSidechain":false,"message":{{"role":"user","content":{}}}}}"#,
        serde_json::Value::String(pointer)
    );
    assert!(Harness::ClaudeCode.names_brief(&session, brief));
}

/// Every other harness takes its brief only typed at it once it is idle,
/// and is answered the line to type: Codex its goal, the rest the pointer.
#[test]
fn every_other_harness_is_handed_its_brief_typed() {
    let brief = "/git/worktrees/7/brief.md";
    for harness in Harness::ALL.into_iter().filter(|h| *h != Harness::ClaudeCode) {
        let line = harness.in_role(harness.program(), &start(None));
        assert_eq!(
            harness.brief_at_start(&line, brief),
            BriefAtStart::Typed(harness.brief_line(brief)),
            "{}",
            harness.key()
        );
    }
    assert!(Harness::Codex.brief_line(brief).starts_with("/goal "));
    assert_eq!(Harness::Pi.brief_line(brief), brief_pointer(brief));
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

/// Each line of a recorded file of hook payloads, as `harness` says its
/// status: its word, its kind, its message, its tool and the end of the
/// call — `None` for an event that changes nothing.
type Read =
    Option<(&'static str, Option<&'static str>, Option<String>, Option<String>, Option<CallEnd>)>;

fn statuses(harness: Harness, recorded: &str) -> Vec<Read> {
    recorded
        .lines()
        .map(|line| {
            let payload: serde_json::Value = serde_json::from_str(line).unwrap();
            match harness.hook_status(&payload).unwrap() {
                Reading::Unchanged => None,
                Reading::Is(Report { status, message, tool, end }) => {
                    Some((status.word(), status.kind(), message, tool, end))
                }
            }
        })
        .collect()
}

fn is(
    word: &'static str,
    kind: Option<&'static str>,
    message: Option<&str>,
    tool: Option<&str>,
    step: Option<CallEnd>,
) -> Read {
    Some((word, kind, message.map(str::to_string), tool.map(str::to_string), step))
}

#[test]
fn recorded_claude_code_hook_events_say_its_session_s_status() {
    let recorded = include_str!("recorded/claude-code-hooks.jsonl");
    let ended = || Some(CallEnd::Result { error: None });
    let bash = Some("Bash");
    let ask = Some("AskUserQuestion");
    let touch = Some("Bash: touch made-by-hook-test");
    let question = Some("Do you prefer tea or coffee?");
    let failed = "Exit code 1\nls: /no/such/dir: No such file or directory";
    let expected: Vec<Read> = vec![
        // An interactive session: a call it asks permission for, then a question.
        is("idle", None, None, None, None),
        is(
            "working",
            None,
            Some("Run the shell command 'touch made-by-hook-test' with the Bash tool."),
            None,
            None,
        ),
        is("working", None, touch, bash, Some(CallEnd::Start)),
        is("blocked", Some("permission"), touch, bash, None),
        None,
        is("working", None, touch, bash, ended()),
        is(
            "done",
            None,
            Some(
                "I ran `touch made-by-hook-test` in the working directory. It completed with no output, so the file should now exist there.",
            ),
            None,
            None,
        ),
        is(
            "working",
            None,
            Some(
                "Use the AskUserQuestion tool to ask me whether I prefer tea or coffee, with those two options.",
            ),
            None,
            None,
        ),
        is("blocked", Some("question"), question, ask, Some(CallEnd::Start)),
        is("blocked", Some("question"), question, ask, None),
        None,
        is("working", None, question, ask, ended()),
        is("done", None, Some("You prefer tea."), None, None),
        is("idle", None, Some("Claude is waiting for your input"), None, None),
        is("idle", None, None, None, None),
        // `claude -p`: a call that succeeds and one that fails.
        is("idle", None, None, None, None),
        is(
            "working",
            None,
            Some(
                "Run the shell command 'echo hello' with the Bash tool, then run the shell command 'ls /no/such/dir' with the Bash tool. Then say done.",
            ),
            None,
            None,
        ),
        is("working", None, Some("Bash: echo hello"), bash, Some(CallEnd::Start)),
        is("working", None, Some("Bash: echo hello"), bash, ended()),
        is("working", None, Some("Bash: ls /no/such/dir"), bash, Some(CallEnd::Start)),
        is(
            "working",
            None,
            Some("Exit code 1"),
            bash,
            Some(CallEnd::Result { error: Some(failed.to_string()) }),
        ),
        is(
            "done",
            None,
            Some(
                "Both commands ran. `echo hello` printed `hello`. `ls /no/such/dir` exited with code 1 and reported `No such file or directory`, as expected for a missing path.",
            ),
            None,
            None,
        ),
        is("idle", None, None, None, None),
        // A model it has no access to, then a key it is refused with.
        is("idle", None, None, None, None),
        is("working", None, Some("say hi"), None, None),
        is(
            "error",
            None,
            Some(
                "There's an issue with the selected model (claude-nonexistent-9). It may not exist or you may not have access to it. Run --model to pick a different model.",
            ),
            None,
            None,
        ),
        is("idle", None, None, None, None),
        is("idle", None, None, None, None),
        is("working", None, Some("say hi"), None, None),
        is("blocked", Some("auth"), Some("Invalid API key · Fix external API key"), None, None),
        is("idle", None, None, None, None),
    ];
    let read = statuses(Harness::ClaudeCode, recorded);
    assert_eq!(read.len(), expected.len());
    for (at, (read, expected)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(read, expected, "line {}", at + 1);
    }
}

#[test]
fn claude_code_and_codex_register_one_command_at_every_status_event() {
    for harness in [Harness::ClaudeCode, Harness::Codex] {
        let events = harness.status_events().unwrap();
        let hooks: serde_json::Value =
            serde_json::from_str(&harness.status_hooks("/bin/tell-status").unwrap()).unwrap();
        let registered = hooks["hooks"].as_object().unwrap();
        assert_eq!(registered.len(), events.len(), "{}", harness.key());
        for event in events {
            assert_eq!(
                registered[*event][0]["hooks"][0]["command"],
                "/bin/tell-status",
                "{}: {event}",
                harness.key()
            );
        }
    }
    let claude = Harness::ClaudeCode.status_events().unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "Notification",
        "Stop",
        "SessionEnd",
    ] {
        assert!(claude.contains(&event), "{event}");
    }
}

#[test]
fn every_other_harness_says_no_status_from_its_hooks() {
    let payload = serde_json::json!({"hook_event_name": "Stop"});
    for harness in Harness::ALL {
        if matches!(harness, Harness::ClaudeCode | Harness::Codex) {
            continue;
        }
        assert_eq!(harness.status_events(), None, "{}", harness.key());
        assert_eq!(harness.status_hooks("/bin/tell-status"), None, "{}", harness.key());
        assert_eq!(harness.hook_status(&payload), None, "{}", harness.key());
    }
}

#[test]
fn codex_hook_events_say_its_session_s_status() {
    let recorded = include_str!("recorded/codex-hooks.jsonl");
    let bash = Some("Bash");
    let expected: Vec<Read> = vec![
        is("idle", None, None, None, None),
        is("working", None, Some("Run cargo test, then remove the build directory."), None, None),
        is("working", None, Some("Bash: cargo test"), bash, Some(CallEnd::Start)),
        // Codex hands a shell call's output alone: no error is said of it.
        is("working", None, Some("Bash: cargo test"), bash, Some(CallEnd::Result { error: None })),
        is("blocked", Some("permission"), Some("Bash: rm -rf build"), bash, None),
        None,
        is("done", None, Some("The tests fail to compile: `x` is not defined."), None, None),
        is("working", None, Some("Try again."), None, None),
        // A turn that ends in an error fires no `Stop`.
        is(
            "error",
            None,
            Some(
                "stream disconnected before completion: error sending request for url (https://chatgpt.com/backend-api/codex/responses)",
            ),
            None,
            None,
        ),
        is("working", None, Some("Never mind."), None, None),
        is("idle", None, None, None, None),
        is("idle", None, None, None, None),
    ];
    let read = statuses(Harness::Codex, recorded);
    assert_eq!(read.len(), expected.len());
    for (at, (read, expected)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(read, expected, "line {}", at + 1);
    }
}

#[test]
fn a_recorded_claude_code_session_says_its_steps_and_the_one_that_failed() {
    let text = include_str!("recorded/claude-code-session-steps.jsonl");
    let steps = Harness::ClaudeCode.steps(text);
    // Three calls answered; the fourth has no result yet, and the request
    // Claude Code keeps beside the session repeats a call, which is no step.
    let said: Vec<(bool, Option<u64>)> = steps.iter().map(|step| (step.error, step.at)).collect();
    assert_eq!(
        said,
        vec![
            (false, Some(1_794_444_749)),
            (false, Some(1_794_444_755)),
            (true, Some(1_794_444_787))
        ]
    );
    for (step, called) in steps.iter().zip([
        r#"Bash {"command":"cat /src/project/.git/convoy-worker-prompt.md"#,
        r#"Bash {"command":"gh issue view 736 --repo sunfmin/control"#,
        r#"Bash {"command":"cat crates/harness/src/line.rs | head -120;"#,
    ]) {
        assert!(step.action.starts_with(called), "{}", step.action);
    }
    assert!(steps[2].result.starts_with("Exit code 1 //! A shell line"), "{}", steps[2].result);
    assert!(steps.iter().all(|step| !step.result.contains('\n') && !step.action.contains('\n')));
    // A sidechain's lines are a child session's, not this one's.
    let child = text.replace("\"isSidechain\":false", "\"isSidechain\":true");
    assert_eq!(Harness::ClaudeCode.steps(&child), Vec::new());
}

/// Each step's action, whether it was an error, and when it came back.
fn steps_of(harness: Harness, text: &str) -> Vec<(String, bool, Option<u64>)> {
    harness.steps(text).into_iter().map(|step| (step.action, step.error, step.at)).collect()
}

/// 2026-11-12T02:00:00Z, when each recorded sample other than Claude Code's starts.
const STEPS_AT: u64 = 1_794_448_800;

#[test]
fn a_codex_rollout_says_its_steps_and_a_command_that_exited_other_than_0() {
    let text = include_str!("recorded/codex-session-steps.jsonl");
    let at = |s: u64| Some(STEPS_AT + s);
    assert_eq!(
        steps_of(Harness::Codex, text),
        vec![
            (r#"exec_command {"cmd":"ls crates","workdir":"/src/project"}"#.into(), false, at(3)),
            (
                r#"exec_command {"cmd":"cargo test -p missing","workdir":"/src/project"}"#.into(),
                true,
                at(9)
            ),
            (
                "apply_patch *** Begin Patch *** Add File: notes.txt +done *** End Patch".into(),
                false,
                at(11)
            ),
            (r#"shell {"command":["bash","-lc","false"]}"#.into(), true, at(13)),
        ]
    );
    let steps = Harness::Codex.steps(text);
    assert!(steps[0].result.ends_with("Output: cli core harness"), "{}", steps[0].result);
    // A sub-agent's rollout is a child session's.
    let child = text.replacen(
        r#""source":"cli""#,
        r#""source":{"subagent":{"thread_spawn":{"parent_thread_id":"p","depth":1}}}"#,
        1,
    );
    assert_ne!(child, text);
    assert_eq!(Harness::Codex.steps(&child), Vec::new());
}

#[test]
fn a_kimi_wire_says_its_steps_and_not_its_sub_agents() {
    let text = include_str!("recorded/kimi-wire-steps.jsonl");
    let steps = Harness::Kimi.steps(text);
    assert_eq!(
        steps_of(Harness::Kimi, text),
        vec![
            (r#"Bash {"command":"git status --short"}"#.into(), false, Some(STEPS_AT + 2)),
            (r#"Bash {"command":"cargo build -p missing"}"#.into(), true, Some(STEPS_AT + 5)),
        ]
    );
    assert_eq!(steps[0].result, "M README.md");
}

#[test]
fn a_pi_session_says_its_steps_and_the_one_that_failed() {
    let text = include_str!("recorded/pi-session-steps.jsonl");
    assert_eq!(
        steps_of(Harness::Pi, text),
        vec![
            (r#"bash {"command":"ls"}"#.into(), false, Some(STEPS_AT + 3)),
            (r#"bash {"command":"npm test"}"#.into(), true, Some(STEPS_AT + 6)),
        ]
    );
    assert_eq!(
        Harness::Pi.steps(text)[1].result,
        r#"npm error Missing script: "test" Command exited with code 1"#
    );
}

#[test]
fn grok_s_updates_say_its_steps_a_refused_one_and_a_command_that_exited_other_than_0() {
    let text = include_str!("recorded/grok-updates-steps.jsonl");
    let at = |s: u64| Some(STEPS_AT + s);
    assert_eq!(
        steps_of(Harness::Grok, text),
        vec![
            (
                r#"run_terminal_command {"command":"wc -l notes.txt","description":"Count the lines"}"#
                    .into(),
                false,
                at(3)
            ),
            (r#"write {"content":"x","path":"notes.txt"}"#.into(), true, at(5)),
            (r#"run_terminal_command {"command":"false"}"#.into(), true, at(7)),
        ]
    );
    assert_eq!(Harness::Grok.steps(text)[0].result, "3 notes.txt");
}

#[test]
fn a_dsh_session_says_its_steps_and_a_command_that_exited_other_than_0() {
    let text = include_str!("recorded/dsh-session-steps.jsonl");
    let at = |s: u64| Some(STEPS_AT + s);
    assert_eq!(
        steps_of(Harness::Dsh, text),
        vec![
            (r#"bash {"command":"ls"}"#.into(), false, at(3)),
            (r#"bash {"command":"make"}"#.into(), true, at(6)),
            (r#"read_file {"path":"missing.txt"}"#.into(), true, at(8)),
        ]
    );
    // A session a sub-agent started is a child's.
    let child =
        text.replacen(r#""delegationDepth":0"#, r#""delegationDepth":1,"origin":"subagent""#, 1);
    assert_ne!(child, text);
    assert_eq!(Harness::Dsh.steps(&child), Vec::new());
}

//! The one test against real pi. Every other test starts a pi worker in a
//! stand-in orca and runs the fleet's extension under a stand-in pi, which
//! proves the fleet builds the right line — not that pi answers it as
//! assumed (docs/adr/0013). This one runs that line for real, in a throwaway
//! role with a throwaway project as its worktree, under a throwaway home that
//! holds everything of a host user's a worker must not load, and has the
//! worker try what its role's own limit and the guard refuse.
//!
//! It is off by default and outside the gate: it needs pi installed. It needs
//! no login and costs nothing: the model is a stand-in here, an endpoint the
//! throwaway home defines as a custom model — which is also what shows that
//! the fleet's own user directory carries what the host knows of providers.
//! Run it by hand when what the fleet adds to the worker launch command
//! changes, or pi is upgraded:
//!
//!   cargo test -p afkconvoy --test cli smoke_pi:: -- --ignored --nocapture
//!
//! `AFKCONVOY_SMOKE_PI_COMMAND` is the worker launch command to try, when it
//! is not the stock one.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use afkconvoy::kept::keep_under;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::role::{Facts, On, Start, Told};
use serde_json::{Value, json};

const ROLE_WORD: &str = "ROLE-7391";
const PROJECT_WORD: &str = "PROJECT-5528";
const USER_WORD: &str = "USER-9920";
const ROLE_SKILL: &str = "convoy-smoke-role-skill";
const USER_SKILLS: [&str; 2] = ["convoy-smoke-user-skill", "convoy-smoke-home-skill"];
const ROLE_SAYS: &str = "ROLE-SAYS-NO-4417";

/// The shell lines the stand-in model has the worker run, in order.
const RUNS: [&str; 3] = ["echo smoke", "echo role-forbidden", "git commit --allow-empty -m smoke"];

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: A throwaway skill of afkconvoy's smoke test\n---\nSay {name}.\n"
    )
}

/// An extension that, loaded, leaves the file `mark`.
fn marking(mark: &Path) -> String {
    format!(
        "import {{ writeFileSync }} from \"node:fs\"\nexport default function () {{\n  \
         writeFileSync({:?}, \"loaded\")\n}}\n",
        mark.display().to_string()
    )
}

/// One chunk of a streamed answer, as an OpenAI-compatible endpoint sends it.
fn chunk(delta: Value, finish: Value) -> String {
    let chunk = json!({"id": "smoke", "object": "chat.completion.chunk", "created": 0,
        "model": "fake-1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    format!("data: {chunk}\n\n")
}

/// Answers one request as the stand-in model: the next of `RUNS` as a call of
/// pi's shell tool while one is left, then a last word.
fn answer(mut stream: TcpStream, asked: &Mutex<Vec<Value>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let ran = messages.iter().filter(|message| message["role"] == "tool").count();
    asked.lock().unwrap().push(request);
    let events = match RUNS.get(ran) {
        Some(line) => {
            let arguments = json!({"command": line}).to_string();
            let call = json!({"index": 0, "id": format!("call_{ran}"), "type": "function",
                "function": {"name": "bash", "arguments": arguments}});
            chunk(json!({"role": "assistant", "tool_calls": [call]}), Value::Null)
                + &chunk(json!({}), json!("tool_calls"))
        }
        None => {
            chunk(json!({"role": "assistant", "content": "done"}), Value::Null)
                + &chunk(json!({}), json!("stop"))
        }
    };
    let events = events + "data: [DONE]\n\n";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\
         Content-Length: {}\r\n\r\n{events}",
        events.len()
    );
}

/// Every text the request `request` holds under the role `role`, joined.
fn said(request: &Value, role: &str) -> String {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let of_role = messages.iter().filter(|message| {
        message["role"] == role || (role == "system" && message["role"] == "developer")
    });
    of_role.map(|message| message["content"].to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
#[ignore = "starts real pi: needs it installed; run by hand"]
fn real_pi_started_in_a_role_sees_the_role_and_the_project_and_nothing_of_the_user() {
    let scratch = tempfile::TempDir::new().unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let (role, project, team) = (root.join("role"), root.join("project"), root.join("team"));
    let (home, cache) = (root.join("home"), root.join("cache"));
    let (theirs, reported) = (root.join("user-extension-loaded"), root.join("report-loaded"));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            answer(stream, &log);
        }
    });

    // The role: its rules, its skill, and a limit of its own in pi's form.
    write(&role, "CLAUDE.md", &format!("# Role rules\n\nThe role word is {ROLE_WORD}.\n"));
    write(&role, &format!(".claude/skills/{ROLE_SKILL}/SKILL.md"), &skill(ROLE_SKILL));
    let limit = format!(
        "export default function (pi) {{\n  pi.on(\"tool_call\", (event) => {{\n    if \
         (String(event.input?.command ?? \"\").includes(\"role-forbidden\")) return {{ block: \
         true, reason: \"{ROLE_SAYS}\" }}\n  }})\n}}\n"
    );
    write(&role, ".pi/extensions/limit.ts", &limit);
    write(
        &project,
        "CLAUDE.md",
        &format!("# Project rules\n\nThe project word is {PROJECT_WORD}.\n"),
    );
    write(&team, "team.md", "a team\n");

    // The host user's own: a custom model, which a worker is to reach; rules,
    // skills in both places pi reads them from, settings and an extension,
    // none of which it is to load; and the worker backend's extension.
    let models = json!({"providers": {"fake": {"baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "api": "openai-completions", "apiKey": "fake-key", "models": [{"id": "fake-1"}]}}});
    write(&home, ".pi/agent/models.json", &models.to_string());
    write(&home, ".pi/agent/AGENTS.md", &format!("The user word is {USER_WORD}.\n"));
    write(&home, ".pi/agent/settings.json", "{\"defaultModel\": \"nothing/at-all\"}\n");
    write(&home, &format!(".pi/agent/skills/{}/SKILL.md", USER_SKILLS[0]), &skill(USER_SKILLS[0]));
    write(&home, &format!(".agents/skills/{}/SKILL.md", USER_SKILLS[1]), &skill(USER_SKILLS[1]));
    write(&home, ".pi/agent/extensions/theirs.ts", &marking(&theirs));
    write(&home, ".pi/agent/extensions/orca-agent-status.ts", &marking(&reported));

    // The very line a bound repo's dispatch hands orca, and what makes it one
    // answer instead of a session.
    let launch = std::env::var("AFKCONVOY_SMOKE_PI_COMMAND")
        .unwrap_or_else(|_| Harness::Pi.stock().to_string());
    let text = |path: &Path| path.display().to_string();
    let kept = keep_under(&cache, Some(&home), Harness::Pi, false).unwrap();
    let start = Start {
        role_dir: &text(&role),
        worktree: &text(&project),
        team_dir: &text(&team),
        model: Some("fake/fake-1"),
        guard: &format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy")),
        kept: &text(&kept),
        host_user: false,
        dsh_profile: afkconvoy_core::team::DshProfile::Fleet,
        host_rules: &[],
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
    let line = format!("{} -p --no-session --offline 'go'", start.in_role(Harness::Pi, &launch));
    let out = Command::new("/bin/sh")
        .args(["-c", &line])
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let asked = asked.lock().unwrap().clone();
    assert!(
        out.status.success() && asked.len() == RUNS.len() + 1,
        "pi did not answer ({} requests): {}\n{}",
        asked.len(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The worker was asked nothing, and is on the stage's model: the stand-in
    // answered it, and said the last word.
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
    assert_eq!(asked[0]["model"], "fake-1");

    // The role's rules and skill are seen, and the worktree's rules…
    let prompt = said(&asked[0], "system");
    assert!(prompt.contains(ROLE_WORD), "the role's rules were not loaded: {prompt}");
    assert!(prompt.contains(ROLE_SKILL), "the role's skill was not loaded: {prompt}");
    assert!(prompt.contains(PROJECT_WORD), "the worktree's rules were not loaded: {prompt}");
    // …and nothing of the user's: no rule, no skill, no extension.
    assert!(!prompt.contains(USER_WORD), "the user's rules were loaded: {prompt}");
    for theirs in USER_SKILLS {
        assert!(!prompt.contains(theirs), "the user's skill {theirs} was loaded: {prompt}");
    }
    assert!(!theirs.exists(), "the user's extension was loaded");
    // The worker backend's own was: it hears of the worker.
    assert!(reported.exists(), "the worker backend's extension was not loaded");

    // What the worker was shown for each call: the first ran, the role's own
    // limit refused the second, and the guard refused the third — reading
    // from the environment that the stage writes nothing.
    let shown = said(asked.last().unwrap(), "tool");
    let once = |what: &str| shown.matches(what).count() == 1;
    assert!(shown.contains("smoke"), "{shown}");
    assert!(once(ROLE_SAYS), "the role's limit refused nothing: {shown}");
    assert!(
        once("the stage `smoke-stage` is marked `writes: false`: `git commit` is refused"),
        "the guard refused nothing: {shown}"
    );

    // Nothing was written to the user's own directory, and the login the
    // fleet's directory holds is the user's, not a copy.
    assert!(!home.join(".pi/agent/sessions").exists(), "pi wrote to the user's directory");
    assert_eq!(
        std::fs::read_link(kept.join("models.json")).unwrap(),
        home.join(".pi/agent/models.json")
    );
}

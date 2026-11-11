//! The real Kimi Code CLI against a local OpenAI-compatible stand-in. This is
//! off by default and outside the gate: it needs `kimi` installed, but no
//! login, network service or account quota. Run with:
//!
//!   cargo test -p afkconvoy --test cli smoke_kimi:: -- --ignored --nocapture

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use afkconvoy::kept::for_worker_under;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::role::{Facts, On, Rules, Start, Told};
use serde_json::{Value, json};

const ROLE_WORD: &str = "ROLE-7391";
const PROJECT_WORD: &str = "PROJECT-5528";
const TEAM_WORD: &str = "TEAM-8846";
const SKILL_WORD: &str = "SKILL-4417";
const BRIEF_WORD: &str = "BRIEF-6603";
const ROLE_SAYS: &str = "ROLE-SAYS-NO-4417";
const RUNS: [&str; 3] =
    ["echo smoke-ran", "echo role-forbidden", "git commit --allow-empty -m smoke"];

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn response(request: &Value, ran: usize) -> String {
    let tool = ran < RUNS.len();
    let (message, delta, finish) = if tool {
        let arguments = json!({"command": RUNS[ran]}).to_string();
        (
            json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("call_{ran}"),
                "type":"function","function":{"name":"Bash","arguments":arguments}}]}),
            json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("call_{ran}"),
                "type":"function","function":{"name":"Bash","arguments":arguments}}]}),
            "tool_calls",
        )
    } else {
        (
            json!({"role":"assistant","content":"done"}),
            json!({"role":"assistant","content":"done"}),
            "stop",
        )
    };
    if request["stream"] == true {
        let first = json!({"id":"chatcmpl-smoke","object":"chat.completion.chunk","created":1,
            "model":"smoke","choices":[{"index":0,"delta":delta}]});
        let last = json!({"id":"chatcmpl-smoke","object":"chat.completion.chunk","created":1,
            "model":"smoke","choices":[{"index":0,"delta":{},"finish_reason":finish}]});
        format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
    } else {
        json!({"id":"chatcmpl-smoke","object":"chat.completion","created":1,"model":"smoke",
            "choices":[{"index":0,"message":message,"finish_reason":finish}],
            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
        .to_string()
    }
}

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
    asked.lock().unwrap().push(request.clone());
    let body = response(&request, ran);
    let content_type =
        if request["stream"] == true { "text/event-stream" } else { "application/json" };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
}

#[test]
#[ignore = "starts real Kimi Code: cargo test -p afkconvoy --test cli smoke_kimi:: -- --ignored"]
fn a_real_kimi_worker_loads_its_role_and_refuses_what_the_role_and_guard_refuse() {
    if Command::new("kimi").arg("--version").output().is_err() {
        eprintln!("skipped: no `kimi` on PATH");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (role, project, team) = (root.join("role"), root.join("project"), root.join("team"));
    let home = root.join("home");
    // A fresh role home on a host that used kimi-cli must not offer its
    // migration menu or eat the brief as the menu's selection.
    write(&home, ".kimi/config.toml", "default_model = 'legacy'\n");
    let hooked = root.join("role-hook-ran");
    let reported = root.join("orca-events");
    let user_hook = root.join("user-hook-ran");

    write(&role, "CLAUDE.md", &format!("The role word is {ROLE_WORD}.\n"));
    write(
        &role,
        ".claude/skills/smoke-skill/SKILL.md",
        &format!(
            "---\nname: smoke-skill\ndescription: The skill word is {SKILL_WORD}.\n---\n\nDo it.\n"
        ),
    );
    std::os::unix::fs::symlink("CLAUDE.md", role.join("AGENTS.md")).unwrap();
    let limit = format!(
        "asked=$(cat); echo ran >> '{}'; case \"$asked\" in *'echo role-forbidden'*) \
         echo {ROLE_SAYS} >&2; exit 2;; esac; exit 0",
        hooked.display()
    );
    write(
        &role,
        ".kimi-code/limits.toml",
        &format!("[[hooks]]\nevent = 'PreToolUse'\ncommand = {:?}\n", limit),
    );
    let own = format!("The project word is {PROJECT_WORD}.\n");
    write(&project, "AGENTS.md", &own);
    let team_rules = format!("The team word is {TEAM_WORD}.\n");
    write(&team, "CLAUDE.md", &team_rules);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || answer(stream, &log));
        }
    });
    write(
        &home,
        ".kimi-code/config.toml",
        &format!(
            "default_model = 'smoke'\n[providers.smoke]\ntype = 'openai'\n\
             base_url = 'http://127.0.0.1:{port}/v1'\napi_key = 'not-a-secret'\n\
             [models.smoke]\nprovider = 'smoke'\nmodel = 'smoke'\nmax_context_size = 100000\n\
             [[hooks]]\nevent = 'PreToolUse'\ncommand = 'echo bad >> {}'\n",
            user_hook.display()
        ),
    );
    let report = format!(
        "#!/bin/sh\ncat >> '{}'; printf '\\n' >> '{}'\n",
        reported.display(),
        reported.display()
    );
    write(&home, ".orca/agent-hooks/kimi-hook.sh", &report);
    std::fs::set_permissions(
        home.join(".orca/agent-hooks/kimi-hook.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let told = Told {
        repo: Some(Rules { file: &project.join("AGENTS.md").display().to_string(), text: &own }),
        added: Some(Rules {
            file: &team.join("CLAUDE.md").display().to_string(),
            text: &team_rules,
        }),
        own: None,
    };
    let guard = format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy"));
    let (kept, _) = for_worker_under(
        &root.join("cache"),
        Some(&home),
        Harness::Kimi,
        false,
        &role,
        &told,
        &guard,
    )
    .unwrap();
    let start = Start {
        role_dir: &role.display().to_string(),
        worktree: &project.display().to_string(),
        team_dir: &team.display().to_string(),
        model: Some("smoke"),
        guard: &guard,
        kept: &kept,
        host_user: false,
        dsh_profile: afkconvoy_core::team::DshProfile::Fleet,
        host_rules: &[],
        shadowed: &[],
        told,
        facts: Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "smoke-stage" },
            role: "dev",
            skill: "smoke-skill",
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
    let line =
        format!("stty rows 50 cols 160; {}", start.in_role(Harness::Kimi, Harness::Kimi.stock()));
    let mut terminal = Command::new("script");
    if cfg!(target_os = "macos") {
        terminal.args(["-q", "/dev/null", "/bin/sh", "-c", &line]);
    } else {
        terminal.args(["-qec", &line, "/dev/null"]);
    }
    let mut session = terminal
        .env("HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let screen = session.stdout.take().unwrap();
    let screen = std::thread::spawn(move || {
        let mut all = Vec::new();
        let _ = BufReader::new(screen).read_to_end(&mut all);
        String::from_utf8_lossy(&all).into_owned()
    });
    std::thread::sleep(Duration::from_secs(4));
    let mut input = session.stdin.take().unwrap();
    write!(input, "{BRIEF_WORD} go").unwrap();
    input.flush().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    write!(input, "\x1b[13u").unwrap();
    input.flush().unwrap();

    let started = Instant::now();
    while asked.lock().unwrap().len() <= RUNS.len()
        && started.elapsed() < Duration::from_secs(90)
        && session.try_wait().unwrap().is_none()
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(500));
    let _ = session.kill();
    let _ = session.wait();
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), RUNS.len() + 1, "Kimi did not finish: {}", screen.join().unwrap());

    let first = asked[0].to_string();
    for word in [ROLE_WORD, PROJECT_WORD, TEAM_WORD, SKILL_WORD, BRIEF_WORD] {
        assert!(first.contains(word), "{word} was not shown: {first}");
    }
    let last = asked.last().unwrap().to_string();
    assert!(last.contains("smoke-ran"), "{last}");
    assert!(last.contains(ROLE_SAYS), "the role's hook refused nothing: {last}");
    assert!(
        last.contains("the stage `smoke-stage` is marked `writes: false`: `git commit` is refused"),
        "{last}"
    );
    assert!(!last.contains("role-forbidden\\n"), "the role's refused call ran: {last}");
    assert!(!project.join(".git").exists(), "the refused commit ran");
    let marks = std::fs::read_to_string(&hooked).unwrap_or_default();
    assert_eq!(marks.lines().count(), RUNS.len(), "the role hook: {marks:?}");
    assert!(!user_hook.exists(), "the user's hook ran");
    let events = std::fs::read_to_string(&reported).unwrap_or_default();
    assert!(events.contains("SessionStart"), "orca heard nothing: {events}");
}

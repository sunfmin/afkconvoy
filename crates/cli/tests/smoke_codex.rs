//! The tests against real Codex. Every other test starts a Codex worker in a
//! stand-in orca and runs its line under a stand-in Codex, which proves the
//! fleet builds the right line — not that Codex answers it as assumed
//! (docs/adr/0013). These run that line for real, in a throwaway role with a
//! throwaway project as its worktree, under a throwaway home that holds rules
//! and skills of a host user's. One reads which rules and which skills Codex
//! would show the model. The other starts the session a worker is, at a
//! terminal of its own, and has the worker try what its role's own limit and
//! the guard refuse.
//!
//! They are off by default and outside the gate: they need Codex installed.
//! They need no login and cost nothing: the first asks Codex what it would
//! show the model (`codex debug prompt-input`), and calls none; the model of
//! the second is a stand-in here, an endpoint named to Codex on the worker
//! launch command, as a host on a provider of its own names one. Run them by
//! hand when what the fleet adds to the worker launch command changes, or
//! Codex is upgraded:
//!
//!   cargo test -p afkconvoy --test cli smoke_codex:: -- --ignored --nocapture
//!
//! `AFKCONVOY_SMOKE_CODEX_COMMAND` is the worker launch command to try, when
//! it is not the stock one.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use afkconvoy::kept::keep_under;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::role::{Facts, On, Rules, Start, Told};
use serde_json::{Value, json};

const ROLE_WORD: &str = "ROLE-7391";
const ABOVE_WORD: &str = "ABOVE-6102";
const PROJECT_WORD: &str = "PROJECT-5528";
const OTHER_WORD: &str = "OTHER-3310";
const TEAM_WORD: &str = "TEAM-8846";
const USER_WORD: &str = "USER-9920";
const SKILL_WORD: &str = "SKILL-4417";
const HOME_SKILL_WORD: &str = "HOMESKILL-2075";
const BRIEF_WORD: &str = "BRIEF-6603";
const ROLE_SAYS: &str = "ROLE-SAYS-NO-4417";

/// The shell lines the stand-in model has the worker run, in order.
const RUNS: [&str; 3] =
    ["echo smoke-ran", "echo role-forbidden", "git commit --allow-empty -m smoke"];
/// How long a real session is given to make every call of `RUNS`.
const SESSION: Duration = Duration::from_secs(90);

/// A skill named `name`, whose description holds `word`.
fn skill(name: &str, word: &str) -> String {
    format!("---\nname: {name}\ndescription: The skill word is {word}.\n---\n\nDo it.\n")
}

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
#[ignore = "runs real Codex: cargo test -p afkconvoy --test cli smoke_codex:: -- --ignored"]
fn a_real_codex_worker_is_shown_its_role_s_rules_and_skills_the_repo_s_own_and_the_team_rules() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (above, project, team) = (root.join("above"), root.join("project"), root.join("team"));
    let (role, home, cache) = (above.join("role"), root.join("home"), root.join("cache"));

    // A role in a repo whose root holds rules too, as a role opened by hand
    // in its subdirectory would be: a worker is given the role's alone.
    write(&role, "CLAUDE.md", &format!("# Role rules\n\nThe role word is {ROLE_WORD}.\n"));
    write(&above, "CLAUDE.md", &format!("The word above is {ABOVE_WORD}.\n"));
    write(&above, ".git/HEAD", "ref: refs/heads/main\n");
    // The role's skill is where every harness's is, and the link is the one
    // the fleet's copy of a role holds for Codex (`Copies::role`).
    write(&role, ".claude/skills/smoke-skill/SKILL.md", &skill("smoke-skill", SKILL_WORD));
    std::fs::create_dir_all(role.join(".agents")).unwrap();
    std::os::unix::fs::symlink("../.claude/skills", role.join(".agents/skills")).unwrap();
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
    // A skill of the host user's home, which Codex finds wherever its user
    // directory is, and one of their Codex's own, which it does not.
    write(&home, ".agents/skills/theirs/SKILL.md", &skill("theirs", HOME_SKILL_WORD));
    write(&home, ".codex/skills/theirs-too/SKILL.md", &skill("theirs-too", USER_WORD));

    let launch = std::env::var("AFKCONVOY_SMOKE_CODEX_COMMAND")
        .unwrap_or_else(|_| Harness::Codex.stock().to_string());
    let text = |path: &Path| path.display().to_string();
    let kept = keep_under(&cache, Some(&home), Harness::Codex, false).unwrap();
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
        host_user: false,
        dsh_profile: afkconvoy_core::team::DshProfile::Fleet,
        host_rules: &[],
        shadowed: &[],
        told: Told {
            repo: Some(Rules { file: &text(&held), text: &own }),
            added: Some(Rules { file: &text(&team.join("CLAUDE.md")), text: &team_rules }),
            own: None,
        },
        facts: Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "smoke-stage" },
            role: "dev",
            skill: "implement",
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
    // The very line a bound repo's dispatch hands orca, and what has Codex
    // say what it would show the model instead of starting a session.
    let line = format!("{} debug prompt-input go", start.in_role(Harness::Codex, &launch));
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
    // The role's skill is found through the link, once; the skills Codex
    // carries and those of the user's Codex are not shown.
    let told = said(&shown, "developer");
    assert_eq!(told.matches(SKILL_WORD).count(), 1, "the role's skill: {told}");
    assert!(!all.contains("skills/.system"), "the skills Codex carries were shown: {all}");
    // The known exception (docs/adr/0013): Codex has no way to be told to
    // leave out the skills of the host user's home, and loads them. Were
    // this to fail, Codex has learned one, and the fleet should use it.
    assert!(told.contains(HOME_SKILL_WORD), "the skills of the user's home: {told}");
}

/// One event of a streamed answer, as the Responses API sends it.
fn event(kind: &str, mut body: Value) -> String {
    body["type"] = json!(kind);
    format!("event: {kind}\ndata: {body}\n\n")
}

/// Answers one request as the stand-in model. A turn of the worker's is
/// answered with the next of `RUNS`, as a call of the tool Codex gave the
/// model to run a shell line with, while one is left, then with a last word.
/// Codex asks for other things beside — a title for the session — and each is
/// given a word and kept out of `asked`.
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
    let input = request["input"].as_array().cloned().unwrap_or_default();
    let is_output = |item: &Value| item["type"].as_str().is_some_and(|t| t.ends_with("_output"));
    let ran = input.iter().filter(|item| is_output(item)).count();
    let a_turn = request["text"]["format"].is_null() && request.to_string().contains(BRIEF_WORD);
    if a_turn {
        asked.lock().unwrap().push(request);
    }
    let item = match RUNS.get(ran).filter(|_| a_turn) {
        Some(line) => call(&input, line, ran),
        None => json!({"type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": "done"}]}),
    };
    let usage = json!({"input_tokens": 1, "input_tokens_details": null, "output_tokens": 1,
        "output_tokens_details": null, "total_tokens": 2});
    let events = event("response.created", json!({"response": {"id": "smoke"}}))
        + &event("response.output_item.done", json!({"item": item}))
        + &event("response.completed", json!({"response": {"id": "smoke", "usage": usage}}));
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\
         Content-Length: {}\r\n\r\n{events}",
        events.len()
    );
}

/// A call that runs the shell line `line`, of whichever tool the request
/// whose input is `input` gave the model for it: Codex gives some models a
/// tool that runs a script whose `tools.exec_command` runs a line, and others
/// a function that runs one.
fn call(input: &[Value], line: &str, ran: usize) -> Value {
    let scripted = input.iter().any(|item| item["type"] == "additional_tools");
    let args = json!({"cmd": line, "login": false});
    if scripted {
        let script = format!(
            "try {{ text(JSON.stringify(await tools.exec_command({args}))) }} \
             catch (e) {{ text(\"threw: \" + String(e)) }}"
        );
        json!({"type": "custom_tool_call", "call_id": format!("call_{ran}"), "name": "exec",
            "input": script})
    } else {
        json!({"type": "function_call", "call_id": format!("call_{ran}"),
            "name": "exec_command", "arguments": args.to_string()})
    }
}

/// What the worker was shown for each call it made, as the request `request`
/// holds it.
fn shown(request: &Value) -> String {
    let input = request["input"].as_array().cloned().unwrap_or_default();
    let outputs = input.iter().filter(|item| item["type"].to_string().contains("_output"));
    outputs.map(|item| item["output"].to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
#[ignore = "starts real Codex: cargo test -p afkconvoy --test cli smoke_codex:: -- --ignored"]
fn a_real_codex_worker_starts_on_its_brief_and_makes_no_call_its_role_or_the_guard_refuses() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (role, project, team) = (root.join("role"), root.join("project"), root.join("team"));
    let (home, cache, hooked) = (root.join("home"), root.join("cache"), root.join("hook-ran"));

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

    // The role: its rules, the skill its stage has it do, and a limit of its
    // own in Codex's form — a hook that leaves a mark each time it is run,
    // and refuses one shell line as Codex takes a refusal: exit 2, and why.
    write(&role, "CLAUDE.md", &format!("# Role rules\n\nThe role word is {ROLE_WORD}.\n"));
    write(&role, ".claude/skills/smoke-skill/SKILL.md", &skill("smoke-skill", SKILL_WORD));
    std::fs::create_dir_all(role.join(".agents")).unwrap();
    std::os::unix::fs::symlink("../.claude/skills", role.join(".agents/skills")).unwrap();
    let limit = format!(
        "asked=$(cat); echo ran >> '{}'; case \"$asked\" in *'echo role-forbidden'*) \
         echo {ROLE_SAYS} >&2; exit 2;; esac; exit 0",
        hooked.display()
    );
    let hooks =
        json!({"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": limit}]}]}});
    write(&role, ".codex/hooks.json", &hooks.to_string());
    let own = format!("# Project rules\n\nThe project word is {PROJECT_WORD}.\n");
    write(&project, "AGENTS.md", &own);
    let team_rules = format!("The team's word is {TEAM_WORD}.\n");
    write(&team, "CLAUDE.md", &team_rules);
    // The host user's own rules and hook, which a worker is given none of.
    write(&home, ".codex/AGENTS.md", &format!("The user word is {USER_WORD}.\n"));
    let theirs = root.join("user-hook-ran");
    let user_hook = format!("echo ran >> '{}'", theirs.display());
    let user_hooks =
        json!({"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": user_hook}]}]}});
    write(&home, ".codex/hooks.json", &user_hooks.to_string());

    // The worker launch command of a host on a provider of its own: the
    // stand-in model, named to Codex on the line.
    let stock = std::env::var("AFKCONVOY_SMOKE_CODEX_COMMAND")
        .unwrap_or_else(|_| Harness::Codex.stock().to_string());
    let launch = format!(
        "{stock} --config 'model_providers.smoke={{name=\"smoke\",\
         base_url=\"http://127.0.0.1:{port}/v1\",wire_api=\"responses\"}}' \
         --config 'model_provider=\"smoke\"'"
    );
    let text = |path: &Path| path.display().to_string();
    let kept = keep_under(&cache, Some(&home), Harness::Codex, false).unwrap();
    let start = Start {
        role_dir: &text(&role),
        worktree: &text(&project),
        team_dir: &text(&team),
        model: None,
        guard: &format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy")),
        kept: &text(&kept),
        host_user: false,
        dsh_profile: afkconvoy_core::team::DshProfile::Fleet,
        host_rules: &[],
        shadowed: &[],
        told: Told {
            repo: Some(Rules { file: &text(&project.join("AGENTS.md")), text: &own }),
            added: Some(Rules { file: &text(&team.join("CLAUDE.md")), text: &team_rules }),
            own: None,
        },
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
    // The very line a bound repo's dispatch hands orca, with the brief as
    // the prompt that starts the session, at a terminal of its own: Codex
    // starts a session at nothing else. `script` is that terminal.
    let line = format!(
        "stty rows 50 cols 160; {} '{BRIEF_WORD} go'",
        start.in_role(Harness::Codex, &launch)
    );
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
    // The session is a worker's: it does not end by itself. It is over here
    // once the stand-in has said its last word, or it has run too long.
    let started = Instant::now();
    while asked.lock().unwrap().len() <= RUNS.len()
        && started.elapsed() < SESSION
        && session.try_wait().unwrap().is_none()
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(500));
    let _ = session.kill();
    let _ = session.wait();
    let asked = asked.lock().unwrap().clone();
    assert_eq!(
        asked.len(),
        RUNS.len() + 1,
        "Codex did not make every call: {}",
        screen.join().unwrap()
    );

    // The prompt on the line started the session: the first thing the model
    // was asked is the brief, with nobody at the terminal.
    let first = &asked[0];
    assert!(said(&first["input"], "user").contains(&format!("{BRIEF_WORD} go")), "{first}");
    // The role's rules were loaded before the model, the repo's own and the
    // team rules told, the skill the stage has the worker do found, and
    // nothing of the user's rules.
    let all = first.to_string();
    assert!(said(&first["input"], "user").contains(ROLE_WORD), "the role's rules: {all}");
    let told = said(&first["input"], "developer");
    assert!(told.contains(PROJECT_WORD) && told.contains(TEAM_WORD), "{told}");
    assert!(told.contains("smoke-skill") && told.contains(SKILL_WORD), "the skill: {told}");
    assert!(!all.contains(USER_WORD), "the user's rules were loaded: {all}");

    // What the worker was shown for each call: the first was made, the
    // role's own limit refused the second, and the guard refused the third —
    // reading from the environment that the stage writes nothing.
    let shown = shown(asked.last().unwrap());
    assert!(shown.contains("smoke-ran"), "{shown}");
    assert!(shown.contains(ROLE_SAYS), "the role's limit refused nothing: {shown}");
    assert!(!shown.contains("role-forbidden\\n"), "the call the role refuses was made: {shown}");
    assert!(
        shown
            .contains("the stage `smoke-stage` is marked `writes: false`: `git commit` is refused"),
        "the guard refused nothing: {shown}"
    );
    // The role's hook was run before each call, and the user's before none.
    let marks = std::fs::read_to_string(&hooked).unwrap_or_default();
    assert_eq!(marks.lines().count(), RUNS.len(), "the role's hook: {marks:?}");
    assert!(!theirs.exists(), "the user's hook was run");
    assert!(!role.join(".git").exists(), "the refused commit was made");
}

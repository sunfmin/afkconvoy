//! Stands in for `orca` in the tests: its repos, worktrees and terminals as
//! one JSON file (`CONVOY_FAKE_ORCA`). Worktrees are real git worktrees.
//!
//! State: `repos` [{id, path}], `root` (where worktrees are made), `worktrees`
//! [{path, branch, linkedIssue, repoId, lastActivityAt, head?, removalError?}], `terminals` [{handle,
//! worktreePath, command, title?, sent, open, agent?, lastOutputAt?, screen?}], `calls`,
//! `envs`: beside each of `calls`, the names of the session's environment it
//! ran in (`fake::State`), `parents`: the pid that made each call, and `from`:
//! the directory each call was run from,
//! which real orca takes the current worktree from where a call names none.
//! A terminal's `line` is the whole line it was started with; `command` is
//! behind the issue export and what starts every one under `nice` with the
//! worktree's own `TMPDIR`: one without it is refused,
//! but for a question (`question::typed`), which is put to `$SHELL -lic` —
//! the stand-in login shell — at once. Its terminal is kept apart, in
//! `questions` [{handle, worktreePath, script, stream, open}]: it shows what
//! that printed between the question's marks (`stream`). A question's
//! terminal is opened in the worktree it names, or with none named in none.
//! Knobs: `stopped` (it is not running: it says so, and every other call
//! fails), `create_fails` [issue…], `rm_fails`, `never_ready`, `send_refused`,
//! `stale_base` (a ref worktrees are cut at instead of the one asked for),
//! `ps_truncated`, `ps_fails`, `close_fails` [worktree paths…],
//! `close_keeps_terminal` (acknowledges close without ending its terminal),
//! `timeout_on_waits` (the numbered waits of each terminal that time out),
//! `start_screen`, `send_screen`, `prompt_stages` (a receipt without a turn
//! start); each terminal can also have `menu` and its own `prompt_stages`.
//! `raw_answers` maps a call's first two words to its stdout, after its effects.
//! A `worktree list` drops a row whose `removalError` is set once its directory
//! is gone, which is how real orca forgets a failed removal. A `worktree rm`
//! whose git remove fails while the directory remains records `removalError`
//! and still answers success. `rm_fails` still refuses before any delete.
//! `notes`: `taken` — every harness's session keeps each line sent as given
//! to its model as the user's; `queued` — only in its input queue;
//! `send-now` — queued on the line, and given to the model when a later
//! send is Enter alone.
//! `startup_working_reports`: a quiet screen while the harness reports a
//! start-up working episode for this many fresh runtime observations.
//!
//! The fleet reads a harness's session, not this stand-in's `working` flag
//! and not the screen. An accepted send writes a closed turn for every
//! harness unless `prompt_stages` omits `turn_started`, `session_turn` is
//! false, or the raw answer for `terminal send` does not contain
//! `turn_started`. `session_turn: true` forces the file. The stamp is the
//! wall clock, or `convoy-started` when that handoff is later: the start
//! proof compares against the handoff and skips a file written before it.
//! `worktree ps` and `terminal show` overwrite that turn from the terminal's
//! agent: `working` is open at `stateStartedAt`, any other state is closed
//! then. No agent leaves the send's turn as it stands.
//! `codex_session_cwd` makes the send's Codex rollout start in that role
//! directory and record the sent brief pointer as its initial user prompt.
//! `claude_session_cwd` files Claude Code's session under that role directory
//! instead of the worktree, with the sent brief pointer as its first prompt.

mod fake;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy::worker_tmp;
use afkconvoy_core::line::niced;
use afkconvoy_core::question;
use afkconvoy_core::role::ISSUE_ENV;
use fake::State;
use serde_json::{Value, json};

fn main() {
    let mut st = State::open("CONVOY_FAKE_ORCA");
    // A host look is a separate doctor process: it must not be counted as
    // another observation by the fleet's tick and panel in their process.
    st.list("parents").push(json!(unsafe { libc::getppid() }));
    assert_eq!(st.argv.last().map(String::as_str), Some("--json"), "{:?}", st.argv);
    let from = std::env::current_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
    st.list("from").push(json!(from));
    if st.argv[0] == "status" {
        status(&st);
    }
    if st.doc["stopped"] == true {
        fail(&st, "runtime_unreachable");
    }
    let argv: Vec<String> = st.argv.clone();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    match argv.as_slice() {
        ["repo", "list", ..] => done(&st, json!({"repos": st.doc["repos"]})),
        ["repo", "add", ..] => repo_add(&mut st),
        ["worktree", "create", ..] => worktree_create(&mut st),
        ["worktree", "list", ..] => worktree_list(&mut st),
        ["worktree", "ps", ..] => worktree_ps(&mut st),
        ["worktree", "rm", ..] => worktree_rm(&mut st),
        ["terminal", "create", ..] => terminal_create(&mut st),
        ["terminal", "wait", ..] => terminal_wait(&mut st),
        ["terminal", "send", ..] => terminal_send(&mut st),
        ["terminal", "list", ..] => terminal_list(&st),
        ["terminal", "read", ..] => terminal_read(&st),
        ["terminal", "show", ..] => terminal_show(&st),
        ["terminal", "close", ..] => terminal_close(&mut st),
        _ => fail(&st, &format!("fake orca: unsupported call {:?}", st.argv)),
    }
}

fn done(st: &State, result: Value) -> ! {
    let what = st.argv.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    let answer = json!({"id": "x", "ok": true, "result": result}).to_string();
    st.finish(st.doc["raw_answers"][&what].as_str().unwrap_or(&answer), 0, "")
}

/// As real orca: asked while it is not running, it answers, and not `ready`.
fn status(st: &State) -> ! {
    let running = st.doc["stopped"] != true;
    let state = if running { "ready" } else { "stopped" };
    done(
        st,
        json!({"app": {"running": running}, "runtime": {"state": state, "reachable": running}}),
    )
}

fn fail(st: &State, code: &str) -> ! {
    let answer = json!({"id": "x", "ok": false, "error": {"code": code, "message": code}});
    st.finish(&answer.to_string(), 1, "")
}

/// The path a `--worktree path:<p>` selector names.
fn selected(st: &State) -> String {
    let selector = st.opt("--worktree").expect("--worktree");
    selector.strip_prefix("path:").unwrap_or_else(|| panic!("{:?}", st.argv)).to_string()
}

fn knows_worktree(st: &State, path: &str) -> bool {
    st.doc["worktrees"].as_array().is_some_and(|rows| rows.iter().any(|row| row["path"] == path))
}

/// As orca's folder projects: a non-git directory can hold terminals too.
fn repo_add(st: &mut State) -> ! {
    let path = st.opt("--path").expect("--path").to_string();
    assert!(Path::new(&path).is_dir(), "no directory at {path}");
    let id = format!("repo-{}", st.list("repos").len() + 1);
    let repo = json!({"id": id, "path": path});
    st.list("repos").push(repo.clone());
    st.list("worktrees")
        .push(json!({"path": path, "repoId": id, "isMainWorktree": true, "isArchived": false}));
    done(st, json!({"repo": repo}))
}

fn open_terminals<'a>(st: &'a State, path: &str) -> Vec<&'a Value> {
    let rows = st.doc["terminals"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    rows.iter().filter(|t| t["open"] == true && t["worktreePath"] == path).collect()
}

fn terminal_at(st: &State) -> Option<usize> {
    let handle = st.opt("--terminal").expect("--terminal");
    let rows = st.doc["terminals"].as_array()?;
    rows.iter().position(|t| t["handle"] == handle && t["open"] == true)
}

fn worktree_create(st: &mut State) -> ! {
    let wanted = st.opt("--repo").unwrap_or("").to_string();
    let repos = st.doc["repos"].as_array().cloned().unwrap_or_default();
    let Some(repo) = repos.iter().find(|r| format!("id:{}", r["id"].as_str().unwrap()) == wanted)
    else {
        fail(st, "selector_not_found")
    };
    assert!(st.has("--no-parent"), "{:?}", st.argv);
    let linked: Option<u64> = st.opt("--issue").map(|n| n.parse().unwrap());
    let refused = st.doc["create_fails"].as_array().cloned().unwrap_or_default();
    if linked.is_some_and(|n| refused.iter().any(|r| r == n)) {
        fail(st, "worktree_create_failed");
    }
    let source = repo["path"].as_str().unwrap();
    let taken =
        fake::git(&["-C", source, "for-each-ref", "--format=%(refname:short)", "refs/heads"]);
    // orca never reuses a branch: a taken name gets a number
    let mut name = st.opt("--name").expect("--name").to_string();
    let mut n = 1;
    while taken.lines().any(|branch| branch == format!("tester/{name}")) {
        n += 1;
        name = format!("{}-{n}", st.opt("--name").unwrap());
    }
    let root = st.doc["root"].as_str().unwrap().to_string();
    let path = format!("{root}/{}", name.replace('/', "-"));
    let branch = format!("tester/{name}");
    let base = st.doc["stale_base"].as_str().or(st.opt("--base-branch")).unwrap().to_string();
    let made = std::process::Command::new("git")
        .args(["-C", source, "worktree", "add", "-q", "-b", &branch, &path, &base])
        .output()
        .unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let activity = st.list("calls").len();
    let head = fake::git(&["-C", &path, "rev-parse", "HEAD"]);
    st.list("worktrees").push(json!({"path": path, "branch": format!("refs/heads/{branch}"),
        "linkedIssue": linked, "repoId": repo["id"], "isMainWorktree": false,
        "isArchived": false, "lastActivityAt": activity, "head": head.clone()}));
    // as real orca: every worktree it cuts opens on a bare shell of its own
    let handle = format!("term-{}", st.list("terminals").len() + 1);
    st.list("terminals").push(json!({"handle": handle, "worktreePath": path, "command": null,
        "sent": [], "open": true}));
    done(
        st,
        json!({"worktree": {"path": path, "branch": format!("refs/heads/{branch}"), "head": head}}),
    )
}

fn worktree_list(st: &mut State) -> ! {
    let mut stored = st.doc["worktrees"].as_array().cloned().unwrap_or_default();
    let before = stored.len();
    // as real orca: a failed removal is forgotten once the directory is gone
    stored.retain(|row| {
        let gone = row["path"].as_str().is_some_and(|path| !Path::new(path).exists());
        let failed = row["removalError"].as_str().is_some_and(|error| !error.is_empty());
        !(gone && failed)
    });
    if stored.len() != before {
        st.doc["worktrees"] = json!(stored);
    }
    let mut rows = st.doc["worktrees"].as_array().cloned().unwrap_or_default();
    for row in &mut rows {
        // as real orca: the branch checked out NOW — none on a detached HEAD.
        // A gitdir already gone leaves a seeded head and branch as they were.
        let Some(path) = row["path"].as_str() else { continue };
        if !Path::new(path).is_dir() {
            continue;
        }
        let path = path.to_string();
        let branch = fake::git(&["-C", &path, "symbolic-ref", "-q", "HEAD"]);
        if !branch.is_empty() {
            row["branch"] = json!(branch);
        }
        let head = fake::git(&["-C", &path, "rev-parse", "HEAD"]);
        if !head.is_empty() {
            row["head"] = json!(head);
        }
    }
    done(st, json!({"worktrees": rows}))
}

fn worktree_ps(st: &mut State) -> ! {
    assert!(st.opt("--limit").is_some(), "{:?}", st.argv);
    if st.doc["ps_fails"] == true {
        fail(st, "runtime_unreachable");
    }
    for terminal in st.list("terminals") {
        if terminal["open"] != true {
            continue;
        }
        if let Some(left) = terminal["startup_working_reports_left"].as_u64() {
            let state = if left > 0 { "working" } else { "idle" };
            terminal["agent"]["state"] = json!(state);
            terminal["agent"]["programStatus"]["state"] = json!(state);
            terminal["startup_working_reports_left"] = json!(left.saturating_sub(1));
            terminal["startup_observations"].as_array_mut().unwrap().push(json!(state));
        }
    }
    let rows = st.doc["worktrees"].as_array().cloned().unwrap_or_default();
    let out: Vec<Value> = rows
        .iter()
        .map(|row| {
            let terminals = open_terminals(st, row["path"].as_str().unwrap());
            let last = terminals.iter().filter_map(|t| t["lastOutputAt"].as_u64()).max();
            let agents: Vec<Value> = terminals
                .iter()
                .filter_map(|terminal| {
                    let mut agent = terminal["agent"].clone();
                    if agent.is_null() {
                        return None;
                    }
                    if agent["paneKey"].is_null() {
                        agent["paneKey"] =
                            json!(format!("{}:pane", terminal["handle"].as_str().unwrap()));
                    }
                    Some(agent)
                })
                .collect();
            json!({"path": row["path"], "linkedIssue": row["linkedIssue"], "repoId": row["repoId"],
                "liveTerminalCount": terminals.len(), "lastOutputAt": last, "agents": agents})
        })
        .collect();
    let truncated = st.doc["ps_truncated"] == true;
    sync_sessions(st);
    done(st, json!({"worktrees": out, "totalCount": out.len(), "truncated": truncated}))
}

fn worktree_rm(st: &mut State) -> ! {
    let path = selected(st);
    if !knows_worktree(st, &path) {
        fail(st, "selector_not_found");
    }
    if st.doc["rm_fails"] == true {
        fail(st, "worktree_busy");
    }
    assert!(st.has("--force"), "{:?}", st.argv);
    let mut failed = false;
    if Path::new(&path).is_dir() {
        let source = st.doc["repos"][0]["path"].as_str().unwrap().to_string();
        let branch = fake::git(&["-C", &path, "symbolic-ref", "-q", "--short", "HEAD"]);
        let removed = std::process::Command::new("git")
            .args(["-C", &source, "worktree", "remove", "--force", &path])
            .output()
            .unwrap();
        // as real orca: git drops the registration and stops on a directory
        // the owner cannot write. The failure is recorded, the row stays, and
        // the call still answers success.
        if !removed.status.success() && Path::new(&path).exists() {
            failed = true;
            let detail = String::from_utf8_lossy(&removed.stderr).trim().to_string();
            for row in st.list("worktrees") {
                if row["path"] == path.as_str() {
                    row["removalError"] =
                        json!(format!("Failed to force delete worktree at {path}. {detail}"));
                }
            }
        } else if !branch.is_empty()
            && fake::git(&["-C", &source, "rev-list", "--count", &branch, "--not", "HEAD"]) == "0"
        {
            // as real orca: the branch goes too, unless it holds what it cannot
            // prove merged — here, a commit the source's checkout does not have
            fake::git(&["-C", &source, "branch", "-D", &branch]);
        }
    }
    if !failed {
        st.list("worktrees").retain(|row| row["path"] != path.as_str());
        for terminal in st.list("terminals") {
            if terminal["worktreePath"] == path.as_str() {
                terminal["open"] = json!(false);
            }
        }
    }
    done(st, json!({"removed": true}))
}

fn terminal_create(st: &mut State) -> ! {
    let given = st.opt("--command").expect("--command").to_string();
    if let Some(script) = question::script(&given) {
        asked(st, script.to_string());
    }
    let path = selected(st);
    if !knows_worktree(st, &path) {
        fail(st, "selector_not_found");
    }
    let niced = niced(&worker_tmp::of(Path::new(&path)).display().to_string(), "");
    let command = given.strip_prefix(&niced).unwrap_or_else(|| panic!("not niced: {given}"));
    let issue_export = format!("export {ISSUE_ENV}=");
    let command = command
        .strip_prefix(&issue_export)
        .and_then(|line| line.split_once(" && "))
        .map_or(command, |(_, command)| command);
    let command = command.to_string();
    let handle = format!("term-{}", st.list("terminals").len() + 1);
    let title = st.opt("--title").map(str::to_string);
    let screen = st.doc["start_screen"].clone();
    st.list("terminals")
        .push(json!({"handle": handle, "worktreePath": path, "command": command, "line": given,
        "title": title, "sent": [], "open": true, "screen": screen}));
    // A real terminal runs the acknowledgement before its harness. Exercise
    // the same lost-create-receipt recovery path without starting a model.
    if matches!(title.as_deref(), Some("afkconvoy watch" | "afkconvoy asks")) {
        let asking = title.as_deref() == Some("afkconvoy asks");
        let journal = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .unwrap()
            .join(if asking {
                "afkconvoy/asking/terminal.json"
            } else {
                "afkconvoy/watch/terminal.json"
            });
        let record: Value = serde_json::from_slice(&std::fs::read(journal).unwrap()).unwrap();
        let owner = if asking { "session" } else { "watch" };
        let transcript = Path::new(record[owner]["transcript"].as_str().unwrap());
        std::fs::create_dir(transcript.with_extension("claim")).unwrap();
        std::fs::write(transcript.with_extension("terminal"), format!("{handle}\n")).unwrap();
    }
    if let Some(reports) = st.doc["startup_working_reports"].as_u64() {
        let terminal = st.list("terminals").last_mut().unwrap();
        terminal["startup_working_reports_left"] = json!(reports);
        terminal["startup_observations"] = json!([]);
        terminal["agent"] = json!({"agentType": "codex", "state": "working",
            "programStatus": {"state": "working"}});
    }
    done(st, json!({"terminal": {"handle": handle, "worktreePath": path}}))
}

/// A question's terminal, which has answered already.
fn asked(st: &mut State, script: String) -> ! {
    let path = st.opt("--worktree").map(|_| selected(st)).unwrap_or_default();
    let in_repo = st.doc["repos"].as_array().unwrap().iter().any(|repo| repo["path"] == path);
    if !path.is_empty() && !in_repo && !knows_worktree(st, &path) {
        fail(st, "selector_not_found");
    }
    let shell = std::env::var("SHELL").expect("SHELL");
    let ran = std::process::Command::new(shell)
        .args(["-lic", &script])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let mut stream = vec!["rc noise".to_string(), "AFKCONVOY_ASKED".to_string()];
    stream.extend(String::from_utf8_lossy(&ran.stdout).lines().map(str::to_string));
    stream.push(String::new());
    stream.push(format!("AFKCONVOY_ANSWERED {}", ran.status.code().unwrap_or(1)));
    let handle = format!("question-{}", st.list("questions").len() + 1);
    st.list("questions").push(json!({"handle": handle, "worktreePath": path, "script": script,
        "stream": stream, "open": true}));
    done(st, json!({"terminal": {"handle": handle, "worktreePath": path}}))
}

fn terminal_wait(st: &mut State) -> ! {
    assert_eq!(st.opt("--for"), Some("tui-idle"), "{:?}", st.argv);
    assert!(st.opt("--timeout-ms").is_some_and(|ms| ms.parse::<u64>().unwrap() > 0));
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    let count = st.doc["terminals"][at]["wait_count"].as_u64().unwrap_or(0) + 1;
    st.doc["terminals"][at]["wait_count"] = json!(count);
    if st.doc["timeout_on_waits"].as_array().is_some_and(|waits| waits.contains(&json!(count))) {
        fail(st, "timeout");
    }
    if st.doc["terminals"][at]["bare_shell"] == true {
        fail(st, "timeout"); // a live shell never satisfies tui-idle
    }
    if st.doc["terminals"][at]["tui_busy"] == true {
        fail(st, "timeout"); // as real orca: a timeout is an error
    }
    // A harness on a busy host whose prompt settles only this long after a
    // wait begins: a shorter wait times out, as real orca's does.
    let asked_ms = st.opt("--timeout-ms").unwrap().parse::<u64>().unwrap();
    if st.doc["settles_after_ms"].as_u64().is_some_and(|ms| asked_ms < ms) {
        fail(st, "timeout");
    }
    if asks_trust(st, at) {
        // as real orca: answered, and a non-zero exit
        let wait = json!({"satisfied": false, "status": "running",
            "blockedReason": "agent-trust-workspace"});
        let answer = json!({"id": "x", "ok": true, "result": {"wait": wait}});
        st.finish(&answer.to_string(), 1, "")
    }
    let ready = st.doc["never_ready"] != true && st.doc["terminals"][at]["menu"] != true;
    let status = if ready { "idle" } else { "timeout" };
    done(st, json!({"wait": {"satisfied": ready, "status": status}}))
}

/// `trust_prompt`: every agent asks whether its folder is trusted before it
/// takes a prompt.
fn asks_trust(st: &State, at: usize) -> bool {
    st.doc["trust_prompt"] == true && st.doc["terminals"][at]["trusted"] != true
}

/// The trust question takes the keys: Down to "Yes, I trust this folder",
/// then — in a send of its own — Enter. Anything else picks "No, exit".
fn answer_trust(st: &mut State, at: usize) -> ! {
    let (text, enter) = (st.opt("--text").map(str::to_string), st.has("--enter"));
    let at_yes = st.doc["terminals"][at]["trust_cursor"] == 1;
    match (text.as_deref(), enter, at_yes) {
        (Some("\x1b[B"), false, _) => st.doc["terminals"][at]["trust_cursor"] = json!(1),
        (None, true, true) => st.doc["terminals"][at]["trusted"] = json!(true),
        _ => {
            st.doc["terminals"][at]["open"] = json!(false);
            fail(st, "terminal_exited")
        }
    }
    done(st, json!({"send": {"accepted": true}}))
}

fn terminal_send(st: &mut State) -> ! {
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    if let Some(reason) = st.doc["prompt_blocked"].as_str() {
        st.finish(
            &json!({"ok": false, "error": {"code": "agent_prompt_blocked", "message": reason}})
                .to_string(),
            1,
            "",
        );
    }
    if st.doc["send_refused"] == true {
        done(st, json!({"send": {"accepted": false}}));
    }
    if asks_trust(st, at) {
        answer_trust(st, at);
    }
    let text = st.opt("--text").unwrap_or("");
    let sent = json!({"text": text, "enter": st.has("--enter")});
    let request = st.opt("--retry-request").map(str::to_string);
    if let Some(request) = &request {
        let previous = st.doc["terminals"][at]["requests"][request].clone();
        if !previous.is_null() {
            assert_eq!(previous["input"], sent, "a replay must use the exact saved input");
            let mut receipt = previous["receipt"].clone();
            receipt["mutation"] = json!({"replayed": true});
            // The first response was lost, so that send wrote no session.
            // Record the turn only when this replay is the answer the CLI
            // receives. A raw stand-in answer is still that lost response.
            let hidden = st.doc["raw_answers"]["terminal send"].is_string();
            if !hidden && receipt_started(&receipt) {
                let cwd = st.doc["terminals"][at]["worktreePath"].as_str().unwrap_or("");
                if !cwd.is_empty() {
                    let cwd = Path::new(cwd);
                    record_turn(cwd, false, send_stamp(cwd));
                }
            }
            done(st, receipt);
        }
    }
    st.doc["terminals"][at]["sent"].as_array_mut().unwrap().push(sent.clone());
    if st.doc["send_screen"].is_array() {
        st.doc["terminals"][at]["screen"] = st.doc["send_screen"].clone();
    }
    let in_startup = st.doc["terminals"][at]["startup_working_reports_left"].is_number()
        && st.doc["terminals"][at]["agent"]["state"] == "working";
    let explicit = st.doc["terminals"][at]["prompt_stages"]
        .as_array()
        .or_else(|| st.doc["prompt_stages"].as_array())
        .cloned();
    let stages = explicit.clone().unwrap_or_else(|| {
        if in_startup {
            vec![json!("input_accepted")]
        } else {
            vec![json!("input_accepted"), json!("turn_started")]
        }
    });
    st.doc["terminals"][at]["last_prompt_stages"] = json!(stages.clone());
    if let Some(deliver) = noted_line(st, at, &sent) {
        let cwd = st.doc["terminals"][at]["worktreePath"].as_str().unwrap_or("");
        record_note(Path::new(cwd), &deliver.0, deliver.1, send_stamp(Path::new(cwd)));
    }
    if record_send(st, at, &stages) {
        let cwd = st.doc["terminals"][at]["worktreePath"].as_str().unwrap_or("");
        if !cwd.is_empty() {
            let cwd = Path::new(cwd);
            let stamp = send_stamp(cwd);
            record_turn(cwd, false, stamp);
            if let Some(role) = st.doc["claude_session_cwd"].as_str() {
                let harness = afkconvoy_core::harness::Harness::ClaudeCode;
                let home = std::env::var("HOME").expect("the world's home");
                let root = Path::new(&home).join(harness.session_place(role).under_home);
                for file in harness.session_files(cwd.to_str().unwrap(), false, stamp) {
                    std::fs::remove_file(root.join(file.path)).unwrap();
                }
                for file in harness.session_files(role, false, stamp) {
                    let path = root.join(file.path);
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let body = file.body.replace("\"brief\"", &sent["text"].to_string());
                    std::fs::write(path, body).unwrap();
                }
            }
            if let Some(role) = st.doc["codex_session_cwd"].as_str() {
                let harness = afkconvoy_core::harness::Harness::Codex;
                let home = std::env::var("HOME").expect("the world's home");
                let root = Path::new(&home).join(harness.session_place(role).under_home);
                let text = sent["text"].as_str().unwrap_or("");
                // A goal line is recorded as codex-cli 0.166.0 records one: the
                // goal's setting said to the model, then the goal itself.
                let prompt = match text.strip_prefix("/goal ") {
                    Some(objective) => format!(
                        "{}\n{}",
                        json!({"type": "response_item", "payload": {
                            "type": "message", "role": "user", "content": [{"type": "input_text",
                            "text": format!("<codex_internal_context source=\"user_goal\">\nUser set the goal: {}\n</codex_internal_context>", json!(objective))}]}}),
                        json!({"type": "event_msg", "payload": {"type": "thread_goal_updated",
                            "goal": {"objective": objective, "status": "active"}}})
                    ),
                    None => json!({"type": "response_item", "payload": {
                        "type": "message", "role": "user", "content": [
                            {"type": "input_text", "text": text}]}})
                    .to_string(),
                };
                for file in harness.session_files(cwd.to_str().unwrap(), false, stamp) {
                    let body = file.body.replace(&json!(cwd).to_string(), &json!(role).to_string());
                    std::fs::write(root.join(file.path), format!("{body}{prompt}\n")).unwrap();
                }
            }
        }
    }
    if st.doc["terminals"][at]["startup_working_reports_left"].is_number() {
        // The brief is taken even during startup, but that episode cannot
        // prove a new turn. This is the receipt failure seen without a spinner.
        st.doc["terminals"][at]["screen"] =
            json!(["Carry out convoy worker brief", "Reading ADRs"]);
        st.doc["terminals"][at].as_object_mut().unwrap().remove("startup_working_reports_left");
        st.doc["terminals"][at]["agent"]["state"] = json!("working");
        st.doc["terminals"][at]["agent"]["programStatus"]["state"] = json!("working");
    }
    let receipt = json!({"send": {"accepted": true, "prompt": {
        "requestId": request.as_deref().unwrap_or("request-1"), "stages": stages,
        "observation": st.doc["prompt_observation"].as_str().unwrap_or("observed")
    }}});
    if let Some(request) = request {
        st.doc["terminals"][at]["requests"][&request] = json!({"input": sent, "receipt": receipt});
    }
    done(st, receipt)
}

/// The line this send leaves in the session, and whether the model was
/// given it. `send-now` keeps a typed line queued until Enter alone.
fn noted_line(st: &State, at: usize, sent: &Value) -> Option<(String, bool)> {
    let mode = st.doc["notes"].as_str()?;
    let line = sent["text"].as_str().unwrap_or("");
    if mode == "send-now" && line.is_empty() && sent["enter"] == true {
        let earlier = st.doc["terminals"][at]["sent"].as_array()?.iter().rev().find_map(|row| {
            let text = row["text"].as_str()?;
            (!text.is_empty()).then(|| text.to_string())
        })?;
        return Some((earlier, true));
    }
    if line.is_empty() {
        return None;
    }
    Some((line.to_string(), mode == "taken"))
}

/// The saved receipt claims the harness started a turn.
fn receipt_started(receipt: &Value) -> bool {
    receipt["send"]["accepted"] == true
        && receipt["send"]["prompt"]["stages"]
            .as_array()
            .is_some_and(|stages| stages.iter().any(|stage| stage == "turn_started"))
}

/// Whether this accepted send should leave a session turn. A raw answer is
/// judged on its text, so a fixture that only accepted input writes nothing
/// even though the handler would otherwise have started a turn.
fn record_send(st: &State, at: usize, stages: &[Value]) -> bool {
    let forced = |value: &Value| match value {
        Value::Bool(set) => Some(*set),
        _ => None,
    };
    if let Some(set) =
        forced(&st.doc["terminals"][at]["session_turn"]).or_else(|| forced(&st.doc["session_turn"]))
    {
        return set;
    }
    if let Some(raw) = st.doc["raw_answers"]["terminal send"].as_str() {
        let accepted = (raw.contains("\"ok\":true") || raw.contains("\"ok\": true"))
            && !raw.contains("\"accepted\":false")
            && !raw.contains("\"accepted\": false");
        return accepted && raw.contains("turn_started");
    }
    let explicit =
        st.doc["terminals"][at]["prompt_stages"].is_array() || st.doc["prompt_stages"].is_array();
    if explicit {
        return stages.iter().any(|stage| stage == "turn_started");
    }
    let in_startup = st.doc["terminals"][at]["startup_working_reports_left"].is_number()
        && st.doc["terminals"][at]["agent"]["state"] == "working";
    !in_startup
}

/// `working` is an open turn at the test's clock. Any other agent state is
/// a turn that ended then. A terminal with no agent keeps the send's turn.
fn sync_sessions(st: &State) {
    let Some(terminals) = st.doc["terminals"].as_array() else { return };
    for terminal in terminals {
        if terminal["open"] != true || !terminal["agent"].is_object() {
            continue;
        }
        let Some(state) = terminal["agent"]["state"].as_str() else { continue };
        if state.is_empty() {
            continue;
        }
        let Some(cwd) = terminal["worktreePath"].as_str() else { continue };
        if cwd.is_empty() {
            continue;
        }
        let at = terminal["agent"]["stateStartedAt"].as_u64().unwrap_or(0) / 1000;
        record_turn(Path::new(cwd), state == "working", at);
    }
}

/// What each harness keeps of `line`, typed at the worker in `cwd`.
fn record_note(cwd: &Path, line: &str, taken: bool, at: u64) {
    let Ok(home) = std::env::var("HOME") else { return };
    if home.is_empty() || cwd.as_os_str().is_empty() {
        return;
    }
    afkconvoy::session::record_note(Path::new(&home), cwd, line, taken, at)
        .unwrap_or_else(|error| panic!("cannot record a note for {}: {error}", cwd.display()));
}

fn record_turn(cwd: &Path, open: bool, at: u64) {
    let Ok(home) = std::env::var("HOME") else { return };
    if home.is_empty() {
        return;
    }
    afkconvoy::session::record(Path::new(&home), cwd, open, at).unwrap_or_else(|error| {
        panic!("cannot record a session turn for {}: {error}", cwd.display())
    });
}

/// When this send's turn is stamped. The proof accepts a turn that started
/// at or after the handoff in `convoy-started`, and it does not read a file
/// last written before that. A fleet clock ahead of the wall clock — a test
/// that moves `--now` past every file mtime — has to be the stamp, or the
/// turn looks older than the handoff.
fn send_stamp(cwd: &Path) -> u64 {
    let wall = now_secs();
    match handed_stamp(cwd) {
        Some(started) if started > wall => started,
        _ => wall,
    }
}

/// The epoch seconds `start` wrote for this worktree, when it did.
fn handed_stamp(cwd: &Path) -> Option<u64> {
    let git = cwd.join(".git");
    let dir = if git.is_file() {
        let text = std::fs::read_to_string(&git).ok()?;
        let raw = text.trim().strip_prefix("gitdir:")?.trim();
        let noted = Path::new(raw);
        if noted.is_absolute() { noted.to_path_buf() } else { cwd.join(noted) }
    } else if git.is_dir() {
        git
    } else {
        return None;
    };
    let text = std::fs::read_to_string(dir.join("convoy-started")).ok()?;
    text.trim().parse().ok()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0)
}

fn terminal_list(st: &State) -> ! {
    let path = selected(st);
    let rows: Vec<Value> = open_terminals(st, &path)
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({"handle": t["handle"], "worktreePath": t["worktreePath"],
                "title": t["title"], "ptyId": t["ptyId"].as_str().unwrap_or(t["handle"].as_str().unwrap()),
                "incarnationId": t["incarnationId"].as_str().unwrap_or("fake-runtime"),
                "connected": t["connected"].as_bool().unwrap_or(true), "writable": true,
                "lastOutputAt": t["lastOutputAt"].as_u64().unwrap_or(i as u64)})
        })
        .collect();
    done(st, json!({"terminals": rows}))
}

/// The question whose terminal `--terminal` names, while it is open.
fn question_at(st: &State) -> Option<usize> {
    let handle = st.opt("--terminal").expect("--terminal");
    let rows = st.doc["questions"].as_array()?;
    rows.iter().position(|t| t["handle"] == handle && t["open"] == true)
}

fn terminal_read(st: &State) -> ! {
    if let Some(at) = question_at(st) {
        // As real orca, without `--screen`: what was printed, line by line.
        assert!(!st.has("--screen") && st.opt("--limit").is_some(), "{:?}", st.argv);
        let tail = st.doc["questions"][at]["stream"].clone();
        done(st, json!({"terminal": {"tail": tail, "source": "stream"}}))
    }
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    let terminal = &st.doc["terminals"][at];
    assert!(st.has("--screen") && st.opt("--limit").is_some(), "{:?}", st.argv);
    if st.doc["read_fails"] == true {
        fail(st, "terminal_not_readable");
    }
    let tail = if terminal["screen"].is_array() { terminal["screen"].clone() } else { json!([]) };
    let source = st.doc["screen_source"].as_str().unwrap_or("screen");
    done(st, json!({"terminal": {"handle": terminal["handle"], "tail": tail, "source": source}}))
}

fn terminal_show(st: &State) -> ! {
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    sync_sessions(st);
    let terminal = &st.doc["terminals"][at];
    done(
        st,
        json!({"terminal": {"handle": terminal["handle"],
        "tabId": terminal["handle"], "leafId": "pane", "lastOutputAt": terminal["lastOutputAt"]}}),
    )
}

fn terminal_close(st: &mut State) -> ! {
    if st.opt("--terminal").is_some() {
        if let Some(at) = question_at(st) {
            st.doc["questions"][at]["open"] = json!(false);
            done(st, json!({"close": {"ptyKilled": true}}))
        }
        let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
        st.doc["terminals"][at]["open"] = json!(false);
        done(st, json!({"close": {"ptyKilled": true}}))
    }
    let path = selected(st);
    assert!(st.has("--all"), "{:?}", st.argv);
    if st.doc["close_fails"].as_array().is_some_and(|paths| paths.contains(&json!(path))) {
        fail(st, "terminal_close_failed");
    }
    if st.doc["close_keeps_terminal"] == true {
        done(st, json!({"closed": true}));
    }
    // a closed agent stays on record, closed; the bare shell leaves nothing behind
    st.list("terminals").retain(|t| !t["command"].is_null() || t["worktreePath"] != path.as_str());
    for terminal in st.list("terminals") {
        if terminal["worktreePath"] == path.as_str() {
            terminal["open"] = json!(false);
        }
    }
    done(st, json!({"closed": true}))
}

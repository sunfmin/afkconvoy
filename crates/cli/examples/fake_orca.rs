//! Stands in for `orca` in the tests: its repos, worktrees and terminals as
//! one JSON file (`CONVOY_FAKE_ORCA`). Worktrees are real git worktrees.
//!
//! State: `repos` [{id, path}], `root` (where worktrees are made), `worktrees`
//! [{path, branch, linkedIssue, repoId, lastActivityAt}], `terminals` [{handle,
//! worktreePath, command, sent, open, agent?, lastOutputAt?, screen?}], `calls`.
//! Knobs: `stopped` (it is not running: it says so, and every other call
//! fails), `create_fails` [issue…], `rm_fails`, `never_ready`, `send_refused`,
//! `stale_base` (a ref worktrees are cut at instead of the one asked for),
//! `ps_truncated`, `ps_fails`.

mod fake;

use std::path::Path;

use fake::State;
use serde_json::{Value, json};

fn main() {
    let mut st = State::open("CONVOY_FAKE_ORCA");
    assert_eq!(st.argv.last().map(String::as_str), Some("--json"), "{:?}", st.argv);
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
        ["worktree", "create", ..] => worktree_create(&mut st),
        ["worktree", "list", ..] => worktree_list(&st),
        ["worktree", "ps", ..] => worktree_ps(&st),
        ["worktree", "rm", ..] => worktree_rm(&mut st),
        ["terminal", "create", ..] => terminal_create(&mut st),
        ["terminal", "wait", ..] => terminal_wait(&st),
        ["terminal", "send", ..] => terminal_send(&mut st),
        ["terminal", "list", ..] => terminal_list(&st),
        ["terminal", "read", ..] => terminal_read(&st),
        ["terminal", "close", ..] => terminal_close(&mut st),
        _ => fail(&st, &format!("fake orca: unsupported call {:?}", st.argv)),
    }
}

fn done(st: &State, result: Value) -> ! {
    st.finish(&json!({"id": "x", "ok": true, "result": result}).to_string(), 0, "")
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
    st.list("worktrees").push(json!({"path": path, "branch": format!("refs/heads/{branch}"),
        "linkedIssue": linked, "repoId": repo["id"], "isMainWorktree": false,
        "isArchived": false, "lastActivityAt": activity}));
    // as real orca: every worktree it cuts opens on a bare shell of its own
    let handle = format!("term-{}", st.list("terminals").len() + 1);
    st.list("terminals").push(json!({"handle": handle, "worktreePath": path, "command": null,
        "sent": [], "open": true}));
    let head = fake::git(&["-C", &path, "rev-parse", "HEAD"]);
    done(
        st,
        json!({"worktree": {"path": path, "branch": format!("refs/heads/{branch}"), "head": head}}),
    )
}

fn worktree_list(st: &State) -> ! {
    let mut rows = st.doc["worktrees"].as_array().cloned().unwrap_or_default();
    for row in &mut rows {
        // as real orca: the branch checked out NOW — none on a detached HEAD
        let path = row["path"].as_str().unwrap().to_string();
        if Path::new(&path).is_dir() {
            row["branch"] = json!(fake::git(&["-C", &path, "symbolic-ref", "-q", "HEAD"]));
        }
    }
    done(st, json!({"worktrees": rows}))
}

fn worktree_ps(st: &State) -> ! {
    assert!(st.opt("--limit").is_some(), "{:?}", st.argv);
    if st.doc["ps_fails"] == true {
        fail(st, "runtime_unreachable");
    }
    let rows = st.doc["worktrees"].as_array().cloned().unwrap_or_default();
    let out: Vec<Value> = rows
        .iter()
        .map(|row| {
            let terminals = open_terminals(st, row["path"].as_str().unwrap());
            let last = terminals.iter().filter_map(|t| t["lastOutputAt"].as_u64()).max();
            let agents: Vec<&Value> =
                terminals.iter().map(|t| &t["agent"]).filter(|agent| !agent.is_null()).collect();
            json!({"path": row["path"], "linkedIssue": row["linkedIssue"], "repoId": row["repoId"],
                "liveTerminalCount": terminals.len(), "lastOutputAt": last, "agents": agents})
        })
        .collect();
    let truncated = st.doc["ps_truncated"] == true;
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
    if Path::new(&path).is_dir() {
        let source = st.doc["repos"][0]["path"].as_str().unwrap().to_string();
        let branch = fake::git(&["-C", &path, "symbolic-ref", "-q", "--short", "HEAD"]);
        fake::git(&["-C", &source, "worktree", "remove", "--force", &path]);
        // as real orca: the branch goes too, unless it holds what it cannot
        // prove merged — here, a commit the source's checkout does not have
        let held = ["-C", &source, "rev-list", "--count", &branch, "--not", "HEAD"];
        if !branch.is_empty() && fake::git(&held) == "0" {
            fake::git(&["-C", &source, "branch", "-D", &branch]);
        }
    }
    st.list("worktrees").retain(|row| row["path"] != path.as_str());
    for terminal in st.list("terminals") {
        if terminal["worktreePath"] == path.as_str() {
            terminal["open"] = json!(false);
        }
    }
    done(st, json!({"removed": true}))
}

fn terminal_create(st: &mut State) -> ! {
    let path = selected(st);
    if !knows_worktree(st, &path) {
        fail(st, "selector_not_found");
    }
    let command = st.opt("--command").expect("--command").to_string();
    let handle = format!("term-{}", st.list("terminals").len() + 1);
    st.list("terminals").push(json!({"handle": handle, "worktreePath": path, "command": command,
        "sent": [], "open": true}));
    done(st, json!({"terminal": {"handle": handle, "worktreePath": path}}))
}

fn terminal_wait(st: &State) -> ! {
    assert_eq!(st.opt("--for"), Some("tui-idle"), "{:?}", st.argv);
    assert!(st.opt("--timeout-ms").is_some_and(|ms| ms.parse::<u64>().unwrap() > 0));
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    if st.doc["terminals"][at]["tui_busy"] == true {
        fail(st, "timeout"); // as real orca: a timeout is an error
    }
    if asks_trust(st, at) {
        // as real orca: answered, and a non-zero exit
        let wait = json!({"satisfied": false, "status": "running",
            "blockedReason": "agent-trust-workspace"});
        let answer = json!({"id": "x", "ok": true, "result": {"wait": wait}});
        st.finish(&answer.to_string(), 1, "")
    }
    let ready = st.doc["never_ready"] != true;
    let status = if ready { "idle" } else { "timeout" };
    done(st, json!({"wait": {"satisfied": ready, "status": status}}))
}

/// `trust_prompt`: every agent asks whether its folder is trusted before it
/// takes a prompt. `trust_sticks`: and asks again however it is answered.
fn asks_trust(st: &State, at: usize) -> bool {
    st.doc["trust_prompt"] == true && st.doc["terminals"][at]["trusted"] != true
}

/// The trust question takes the keys: Down to "Yes, I trust this folder",
/// then — in a send of its own — Enter. Anything else picks "No, exit".
fn answer_trust(st: &mut State, at: usize) -> ! {
    let (text, enter) = (st.opt("--text").map(str::to_string), st.has("--enter"));
    let sticks = st.doc["trust_sticks"] == true;
    let at_yes = st.doc["terminals"][at]["trust_cursor"] == 1;
    match (text.as_deref(), enter, at_yes) {
        (Some("\x1b[B"), false, _) => st.doc["terminals"][at]["trust_cursor"] = json!(1),
        (None, true, true) => st.doc["terminals"][at]["trusted"] = json!(!sticks),
        _ => {
            st.doc["terminals"][at]["open"] = json!(false);
            fail(st, "terminal_exited")
        }
    }
    done(st, json!({"send": {"accepted": true}}))
}

fn terminal_send(st: &mut State) -> ! {
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    if st.doc["send_refused"] == true {
        done(st, json!({"send": {"accepted": false}}));
    }
    if asks_trust(st, at) {
        answer_trust(st, at);
    }
    let sent = json!({"text": st.opt("--text").expect("--text"), "enter": st.has("--enter")});
    st.doc["terminals"][at]["sent"].as_array_mut().unwrap().push(sent);
    done(st, json!({"send": {"accepted": true}}))
}

fn terminal_list(st: &State) -> ! {
    let path = selected(st);
    let rows: Vec<Value> = open_terminals(st, &path)
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({"handle": t["handle"], "worktreePath": t["worktreePath"],
                "connected": t["connected"].as_bool().unwrap_or(true), "writable": true,
                "lastOutputAt": t["lastOutputAt"].as_u64().unwrap_or(i as u64)})
        })
        .collect();
    done(st, json!({"terminals": rows}))
}

fn terminal_read(st: &State) -> ! {
    let Some(at) = terminal_at(st) else { fail(st, "terminal_handle_stale") };
    assert!(st.has("--screen") && st.opt("--limit").is_some(), "{:?}", st.argv);
    if st.doc["read_fails"] == true {
        fail(st, "terminal_not_readable");
    }
    let terminal = &st.doc["terminals"][at];
    let tail = if terminal["screen"].is_array() { terminal["screen"].clone() } else { json!([]) };
    done(st, json!({"terminal": {"handle": terminal["handle"], "tail": tail, "source": "screen"}}))
}

fn terminal_close(st: &mut State) -> ! {
    let path = selected(st);
    assert!(st.has("--all"), "{:?}", st.argv);
    // a closed agent stays on record, closed; the bare shell leaves nothing behind
    st.list("terminals").retain(|t| !t["command"].is_null() || t["worktreePath"] != path.as_str());
    for terminal in st.list("terminals") {
        if terminal["worktreePath"] == path.as_str() {
            terminal["open"] = json!(false);
        }
    }
    done(st, json!({"closed": true}))
}

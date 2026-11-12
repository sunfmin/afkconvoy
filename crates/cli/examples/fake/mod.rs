//! What the two stand-ins share: one JSON state file each, held under a lock
//! from reading it to replacing it, because a tick calls them several at a time.
#![allow(dead_code)]

use std::fs::File;
use std::path::PathBuf;
use std::process::{Command, exit};

use serde_json::Value;

pub struct State {
    path: PathBuf,
    _lock: File,
    pub doc: Value,
    pub argv: Vec<String>,
}

impl State {
    /// Opens the state file the environment variable `var` names and logs this call.
    pub fn open(var: &str) -> State {
        State::at(named_by(var))
    }

    /// Opens the state of the repo this call is run in, and logs this call:
    /// the file named in the first `.git/<marker>` found from its working
    /// directory up, else the one `var` names. One host runner holds the
    /// fleets of several repos, each with a stand-in of its own.
    pub fn open_routed(var: &str, marker: &str) -> State {
        let here = std::env::current_dir().unwrap_or_default();
        let routed = here
            .ancestors()
            .find_map(|dir| std::fs::read_to_string(dir.join(".git").join(marker)).ok());
        State::at(routed.map_or_else(|| named_by(var), |file| PathBuf::from(file.trim())))
    }

    /// Opens the state file at `path`, and logs this call there.
    pub fn at(path: PathBuf) -> State {
        let lock = File::create(path.with_extension("lock")).expect("lock file");
        lock.lock().expect("lock");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("state file"))
            .expect("state file is JSON");
        let argv: Vec<String> = std::env::args().skip(1).collect();
        // `slow_ms`: how long every call takes, for a test that needs time to pass.
        if let Some(ms) = doc["slow_ms"].as_u64() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        let mut state = State { path, _lock: lock, doc, argv };
        let call = Value::from(state.argv.clone());
        state.list("calls").push(call);
        state.list("envs").push(Value::from(session_names()));
        state
    }

    /// The array at `key`, created empty when the state has none.
    pub fn list(&mut self, key: &str) -> &mut Vec<Value> {
        let slot = &mut self.doc[key];
        if !slot.is_array() {
            *slot = Value::Array(vec![]);
        }
        slot.as_array_mut().unwrap()
    }

    /// The value after the flag `name`, when the call has it.
    pub fn opt(&self, name: &str) -> Option<&str> {
        let at = self.argv.iter().position(|a| a == name)?;
        self.argv.get(at + 1).map(String::as_str)
    }

    pub fn opts(&self, name: &str) -> Vec<String> {
        let mut found = vec![];
        for (i, a) in self.argv.iter().enumerate() {
            if a == name {
                found.extend(self.argv.get(i + 1).cloned());
            }
        }
        found
    }

    pub fn has(&self, flag: &str) -> bool {
        self.argv.iter().any(|a| a == flag)
    }

    pub fn save(&self) {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, self.doc.to_string()).expect("write state");
        std::fs::rename(&tmp, &self.path).expect("replace state");
    }

    /// Saves the state and ends the call.
    pub fn finish(&self, out: &str, code: i32, err: &str) -> ! {
        self.save();
        if !out.is_empty() {
            println!("{out}");
        }
        eprint!("{err}");
        exit(code)
    }
}

/// The file the environment variable `var` names.
fn named_by(var: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| panic!("{var} is not set")))
}

/// The names in this call's environment that a session sets for itself —
/// Claude Code's and orca's — and the one a world's login profile alone sets
/// (`AFKCONVOY_TEST_LOGIN`), and XDG_CONFIG_HOME, which a world's login
/// profile never sets, sorted: whose environment the call ran in. Kept in
/// `envs`, one a call, beside `calls`.
fn session_names() -> Vec<String> {
    let names = std::env::vars_os().filter_map(|(name, _)| name.into_string().ok());
    let session = |name: &String| {
        ["CLAUDE_CODE_", "CLAUDECODE", "ORCA_"].iter().any(|set| name.starts_with(set))
            || ["AFKCONVOY_TEST_LOGIN", "XDG_CONFIG_HOME"].contains(&name.as_str())
    };
    let mut names: Vec<String> = names.filter(session).collect();
    names.sort();
    names
}

/// git's stdout, trimmed; empty when the command failed.
pub fn git(args: &[&str]) -> String {
    let out = Command::new("git").args(args).output().expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Whether a git command succeeded.
pub fn ok(args: &[&str]) -> bool {
    Command::new("git").args(args).output().expect("git runs").status.success()
}

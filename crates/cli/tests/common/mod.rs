//! The world a test runs afkconvoy in: a bare repo standing in for the remote,
//! a clone of it, and stand-ins for `gh` and `orca` first on PATH.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

pub const BASE: &str = "main";

/// The config every world starts with, committed on the base branch.
pub const CONFIG: &str =
    "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nconcurrency: 2\n```\n";

pub struct World {
    _root: TempDir,
    pub root: PathBuf,
    /// The remote: every clone's `origin`.
    pub bare: PathBuf,
    /// The checkout the fleet runs in.
    pub clone: PathBuf,
    bin: PathBuf,
    gh_file: PathBuf,
    orca_file: PathBuf,
}

/// One answer of the CLI: the JSON object it printed, and its exit code.
#[derive(Debug)]
pub struct Answer {
    pub json: Value,
    pub code: i32,
    pub stderr: String,
}

impl Answer {
    /// The answer of a call that must have succeeded.
    pub fn ok(self) -> Value {
        assert_eq!(self.code, 0, "{}", self.json);
        self.json
    }

    /// The error of a call that must have failed.
    pub fn error(self) -> String {
        assert_ne!(self.code, 0, "expected a failure, got {}", self.json);
        self.json["error"].as_str().expect("a failure prints {\"error\": …}").to_string()
    }
}

impl World {
    pub fn new() -> World {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (bare, clone, bin) = (root.join("remote.git"), root.join("clone"), root.join("bin"));
        git(&root, &["init", "-q", "--bare", "-b", BASE, bare.to_str().unwrap()]);
        git(&root, &["clone", "-q", bare.to_str().unwrap(), clone.to_str().unwrap()]);
        std::fs::write(clone.join("README.md"), "widgets\n").unwrap();
        std::fs::create_dir_all(clone.join("docs/agents")).unwrap();
        std::fs::write(clone.join("docs/agents/afkconvoy.md"), CONFIG).unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-q", "-m", "initial"]);
        git(&clone, &["push", "-q", "origin", BASE]);

        std::fs::create_dir(&bin).unwrap();
        let built = Path::new(env!("CARGO_BIN_EXE_afkconvoy")).parent().unwrap().join("examples");
        for (name, example) in [("gh", "fake_gh"), ("orca", "fake_orca")] {
            std::os::unix::fs::symlink(built.join(example), bin.join(name)).unwrap();
        }
        let world = World {
            gh_file: root.join("gh.json"),
            orca_file: root.join("orca.json"),
            _root: dir,
            root,
            bare,
            clone,
            bin,
        };
        world.write_gh(&json!({"bare": world.bare, "base": BASE, "issues": [], "prs": [],
            "comments": {}, "labels": ["ready-for-agent"], "calls": []}));
        world.write_orca(&json!({"repos": [{"id": "repo-1", "path": world.clone}],
            "root": world.root.join("wt"), "worktrees": [], "terminals": [], "calls": []}));
        world
    }

    /// A command with the stand-ins first on PATH and nothing of the caller's
    /// git or GitHub setup leaking in.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>, cwd: &Path) -> Command {
        let mut cmd = Command::new(program);
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        cmd.current_dir(cwd)
            .env("PATH", path)
            .env("CONVOY_FAKE_GH", &self.gh_file)
            .env("CONVOY_FAKE_ORCA", &self.orca_file)
            .envs(GIT_ENV)
            .env_remove("ANTHROPIC_BASE_URL");
        cmd
    }

    /// Runs the real CLI in the fleet's checkout.
    pub fn afk(&self, args: &[&str]) -> Answer {
        self.afk_in(&self.clone, args)
    }

    pub fn afk_in(&self, cwd: &Path, args: &[&str]) -> Answer {
        let out = self.command(env!("CARGO_BIN_EXE_afkconvoy"), cwd).args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "not one JSON object ({e}): {stdout:?}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        Answer { json, code: out.status.code().unwrap(), stderr }
    }

    /// Sets the repo's gate command: committed on the base branch and pushed,
    /// as a repo's config is.
    pub fn set_gate(&self, command: &str) {
        let config =
            format!("```yaml\nbase_branch: main\ngate_command: {command:?}\nconcurrency: 2\n```\n");
        std::fs::write(self.clone.join("docs/agents/afkconvoy.md"), config).unwrap();
        git(&self.clone, &["commit", "-q", "-am", "set the gate"]);
        git(&self.clone, &["push", "-q", "origin", BASE]);
    }

    /// Runs a command that prints lines before its answer → (the lines, the answer).
    pub fn afk_lines(&self, args: &[&str]) -> (Vec<String>, Value) {
        let out =
            self.command(env!("CARGO_BIN_EXE_afkconvoy"), &self.clone).args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines: Vec<String> = stdout.lines().map(str::to_string).collect();
        let last = lines.pop().unwrap_or_default();
        let answer = serde_json::from_str(&last).unwrap_or_else(|e| panic!("{e}: {stdout}"));
        assert!(out.status.success(), "{stdout}");
        (lines, answer)
    }

    /// Marks an issue closed on the stand-in GitHub.
    pub fn close_issue(&self, number: u64) {
        self.with_gh(|gh| {
            for issue in gh["issues"].as_array_mut().unwrap() {
                if issue["number"] == number {
                    issue["state"] = json!("closed");
                }
            }
        });
    }

    /// The issues claimed on the remote, with the instance holding each.
    pub fn claims(&self) -> Vec<(u64, String)> {
        let scan = self.afk(&["scan"]).ok();
        let rows = scan["claims"].as_array().unwrap();
        rows.iter()
            .map(|c| {
                (c["number"].as_u64().unwrap(), c["instance"].as_str().unwrap_or("").to_string())
            })
            .collect()
    }

    pub fn gh(&self) -> Value {
        read(&self.gh_file)
    }

    /// Changes the stand-in GitHub's state in place.
    pub fn with_gh(&self, change: impl FnOnce(&mut Value)) {
        let mut state = self.gh();
        change(&mut state);
        self.write_gh(&state);
    }

    /// Adds an open issue carrying `labels`.
    pub fn issue(&self, number: u64, labels: &[&str]) {
        self.with_gh(|gh| {
            let issue = json!({"number": number, "title": format!("issue {number}"),
                "labels": labels, "state": "open"});
            gh["issues"].as_array_mut().unwrap().push(issue);
        });
    }

    /// Adds an open PR from `branch` whose body closes issue `closes`.
    pub fn pr(&self, number: u64, closes: u64, branch: &str) {
        self.with_gh(|gh| {
            let pr = json!({"number": number, "title": format!("pr {number}"), "headRefName": branch, "headRefOid": "",
                "body": format!("Closes #{closes}\n"), "state": "open"});
            gh["prs"].as_array_mut().unwrap().push(pr);
        });
    }

    /// Every `gh` call made since the last look, as argv lists.
    pub fn gh_calls(&self) -> Vec<Vec<String>> {
        let calls = serde_json::from_value(self.gh()["calls"].clone()).unwrap();
        self.with_gh(|gh| gh["calls"] = json!([]));
        calls
    }

    pub fn write_gh(&self, state: &Value) {
        std::fs::write(&self.gh_file, state.to_string()).unwrap();
    }

    pub fn orca(&self) -> Value {
        read(&self.orca_file)
    }

    /// Changes the stand-in orca's state in place.
    pub fn with_orca(&self, change: impl FnOnce(&mut Value)) {
        let mut state = self.orca();
        change(&mut state);
        self.write_orca(&state);
    }

    /// Every `orca` call made since the last look, without the trailing `--json`.
    pub fn orca_calls(&self) -> Vec<Vec<String>> {
        let mut calls: Vec<Vec<String>> =
            serde_json::from_value(self.orca()["calls"].clone()).unwrap();
        for call in &mut calls {
            call.pop();
        }
        self.with_orca(|orca| orca["calls"] = json!([]));
        calls
    }

    /// The bodies of the comments on an issue or a PR, oldest first.
    pub fn comments(&self, number: u64) -> Vec<String> {
        let rows =
            self.gh()["comments"][number.to_string()].as_array().cloned().unwrap_or_default();
        rows.iter().map(|c| c["body"].as_str().unwrap().to_string()).collect()
    }

    /// Commits a file on the base branch straight on the remote: someone else landed work.
    pub fn land_elsewhere(&self, file: &str) -> String {
        let other = self.root.join(format!("other-{file}"));
        git(&self.root, &["clone", "-q", self.bare.to_str().unwrap(), other.to_str().unwrap()]);
        std::fs::write(other.join(file), "landed elsewhere\n").unwrap();
        git(&other, &["add", "."]);
        git(&other, &["commit", "-q", "-m", file]);
        git(&other, &["push", "-q", "origin", BASE]);
        git(&other, &["rev-parse", "HEAD"])
    }

    pub fn write_orca(&self, state: &Value) {
        std::fs::write(&self.orca_file, state.to_string()).unwrap();
    }

    /// Has the remote log every ref pushed to it, in order, and reject the
    /// pushes of a ref whose name contains `reject` (when given).
    pub fn watch_pushes(&self, reject: Option<&str>) {
        let hook = self.bare.join("hooks/pre-receive");
        let log = self.root.join("pushes.log");
        let reject = reject.unwrap_or("\x01never");
        let script = format!(
            "#!/bin/sh\nwhile read old new ref; do\n  echo \"$ref\" >> '{}'\n  \
             case \"$ref\" in *{reject}*) echo 'rejected by the test' >&2; exit 1;; esac\ndone\n",
            log.display()
        );
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The refs pushed to the remote since `watch_pushes`, in order.
    pub fn pushes(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.root.join("pushes.log")).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }

    /// git against the remote itself.
    pub fn remote(&self, args: &[&str]) -> String {
        git(&self.bare, args)
    }
}

/// An identity for the commits a test makes, and no config of the machine's.
const GIT_ENV: [(&str, &str); 6] = [
    ("GIT_AUTHOR_NAME", "tester"),
    ("GIT_AUTHOR_EMAIL", "tester@example.com"),
    ("GIT_COMMITTER_NAME", "tester"),
    ("GIT_COMMITTER_EMAIL", "tester@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
];

/// Runs git in `cwd` and returns its trimmed stdout; a failure fails the test.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(cwd).args(args).envs(GIT_ENV).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn read(file: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

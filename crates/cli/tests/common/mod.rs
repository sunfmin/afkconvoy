//! The world a test runs afkconvoy in: a bare repo standing in for the remote,
//! a clone of it, and stand-ins for `gh` and `orca` first on PATH.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

pub const BASE: &str = "main";

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
        world.write_orca(&json!({"calls": []}));
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
        Answer { json, code: out.status.code().unwrap() }
    }

    pub fn gh(&self) -> Value {
        read(&self.gh_file)
    }

    pub fn write_gh(&self, state: &Value) {
        std::fs::write(&self.gh_file, state.to_string()).unwrap();
    }

    pub fn orca(&self) -> Value {
        read(&self.orca_file)
    }

    pub fn write_orca(&self, state: &Value) {
        std::fs::write(&self.orca_file, state.to_string()).unwrap();
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

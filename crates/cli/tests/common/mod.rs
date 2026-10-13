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
    "```yaml\nbase_branch: main\ngate_command: ./gate.sh\ngate_seconds: 600\nconcurrency: 2\n```\n";

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
        write_script(&bin.join("shell"), SHELL);
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
        let only = self.root.join("path");
        let path = match only.exists() {
            true => only.display().to_string(),
            false => format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
        };
        cmd.current_dir(cwd)
            .env("PATH", path)
            .env("CONVOY_FAKE_GH", &self.gh_file)
            .env("CONVOY_FAKE_ORCA", &self.orca_file)
            .env("SHELL", self.bin.join("shell"))
            .env("XDG_CACHE_HOME", self.cache())
            .env("HOME", self.home())
            .envs(GIT_ENV)
            .env_remove("ANTHROPIC_BASE_URL");
        cmd
    }

    /// Where this world's fleet keeps what it caches on its host: the copies
    /// of a team and its roles are under it.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// The home of this world's host user: nothing of the home of whoever
    /// runs the tests is read. It holds nothing but what a test puts there.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
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
        self.set_gate_config(&format!("gate_command: {command:?}\ngate_seconds: 600"));
    }

    /// Commits a config of the gate's keys as `yaml` has them, and pushes it.
    pub fn set_gate_config(&self, yaml: &str) {
        let config = format!("```yaml\nbase_branch: main\n{yaml}\nconcurrency: 2\n```\n");
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

    /// Puts another repo on the stand-in GitHub as `name` (`owner/repo`), with
    /// `files` committed on its default branch → its checkout, to commit more to.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.root.join("github").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", BASE]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        for (file, text) in files {
            write(&dir, file, text);
        }
        if !files.is_empty() {
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "files"]);
        }
        self.with_gh(|gh| gh["repos"][name] = json!(dir));
        dir
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

    /// A worker's worktree for `issue`, cut from the base branch, with one
    /// commit pushed and PR #`issue`0 open for it.
    pub fn worker(&self, issue: u64, file: &str, text: &str) -> PathBuf {
        let dir = self.root.join(format!("wt-{issue}"));
        let branch = format!("issue-{issue}");
        git(&self.clone, &["fetch", "-q", "origin"]);
        let at = dir.to_str().unwrap();
        git(&self.clone, &["worktree", "add", "-q", "-b", &branch, at, "origin/main"]);
        commit(&dir, file, text);
        git(&dir, &["push", "-q", "origin", "HEAD"]);
        self.issue(issue, &["ready-for-agent"]);
        self.pr(issue * 10, issue, &branch);
        dir
    }

    /// Posts a comment on an issue or a PR, as a worker would → its URL.
    pub fn comment(&self, number: u64, body: &str) -> String {
        let mut url = String::new();
        self.with_gh(|gh| {
            let every = gh["comments"].as_object().unwrap().values();
            let id = 2000 + every.map(|rows| rows.as_array().unwrap().len()).sum::<usize>();
            url = format!("https://gh/c/{id}");
            let rows = &mut gh["comments"][number.to_string()];
            if !rows.is_array() {
                *rows = json!([]);
            }
            rows.as_array_mut().unwrap().push(json!({"id": id, "body": body, "html_url": url}));
        });
        url
    }

    /// The labels an issue carries.
    pub fn labels(&self, number: u64) -> Vec<String> {
        let gh = self.gh();
        let rows = gh["issues"].as_array().unwrap();
        let issue = rows.iter().find(|row| row["number"] == number).unwrap();
        serde_json::from_value(issue["labels"].clone()).unwrap()
    }

    /// The issues GitHub records `number` as blocked by.
    pub fn edges(&self, number: u64) -> Vec<u64> {
        let edges = self.gh()["edges"][number.to_string()].clone();
        if edges.is_null() { vec![] } else { serde_json::from_value(edges).unwrap() }
    }

    /// Closes an issue, for GitHub's `reason`.
    pub fn close(&self, number: u64, reason: &str) {
        self.with_gh(|gh| {
            for issue in gh["issues"].as_array_mut().unwrap() {
                if issue["number"] == number {
                    issue["state"] = json!("closed");
                    issue["state_reason"] = json!(reason);
                }
            }
        });
    }

    /// Whether an issue is `open` or `closed`.
    pub fn state(&self, number: u64) -> String {
        let gh = self.gh();
        let rows = gh["issues"].as_array().unwrap();
        let issue = rows.iter().find(|row| row["number"] == number).unwrap();
        issue["state"].as_str().unwrap().to_string()
    }

    /// Every `gh` call made since the last look, as argv lists — all but the
    /// read of the base branch's protection every start makes.
    pub fn gh_calls(&self) -> Vec<Vec<String>> {
        let mut calls: Vec<Vec<String>> =
            serde_json::from_value(self.gh()["calls"].clone()).unwrap();
        calls.retain(|call| !call.iter().any(|arg| arg.ends_with("/protection")));
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

    /// The worktrees the stand-in orca lists, oldest first.
    pub fn worktrees(&self) -> Vec<PathBuf> {
        let orca = self.orca();
        let rows = orca["worktrees"].as_array().unwrap().iter();
        rows.map(|row| PathBuf::from(row["path"].as_str().unwrap())).collect()
    }

    /// Where each terminal the stand-in orca has open is: one path a terminal.
    pub fn open_terminals(&self) -> Vec<String> {
        let orca = self.orca();
        let rows = orca["terminals"].as_array().unwrap().iter();
        rows.filter(|terminal| terminal["open"] == true)
            .map(|terminal| terminal["worktreePath"].as_str().unwrap().to_string())
            .collect()
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

    /// The bodies of the comments on an issue or a PR, oldest first — all but
    /// its status board, which is `board`'s.
    pub fn comments(&self, number: u64) -> Vec<String> {
        self.every_comment(number).into_iter().filter(|body| !is_board(body)).collect()
    }

    /// The status boards an issue carries: one, once it was ever claimed.
    pub fn boards(&self, number: u64) -> Vec<String> {
        self.every_comment(number).into_iter().filter(|body| is_board(body)).collect()
    }

    fn every_comment(&self, number: u64) -> Vec<String> {
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

    /// The refs pushed to the remote since `watch_pushes`, in order — all but
    /// the probes every start pushes and deletes (`probes`).
    pub fn pushes(&self) -> Vec<String> {
        self.every_push().into_iter().filter(|name| !name.contains("probe-")).collect()
    }

    /// The probe refs pushed to the remote since `watch_pushes`: each once
    /// when pushed and once when deleted.
    pub fn probes(&self) -> Vec<String> {
        self.every_push().into_iter().filter(|name| name.contains("probe-")).collect()
    }

    fn every_push(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.root.join("pushes.log")).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }

    /// Has every `git` call afkconvoy makes from here on logged (`git_calls`).
    pub fn watch_git(&self) {
        let path = std::env::var_os("PATH").unwrap();
        let git = std::env::split_paths(&path).map(|dir| dir.join("git")).find(|git| git.is_file());
        let (git, log) = (git.expect("git is on PATH"), self.root.join("git.log"));
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            git.display()
        );
        write_script(&self.bin.join("git"), &script);
    }

    /// The `git` calls afkconvoy made since `watch_git`, each as its command
    /// line, in order.
    pub fn git_calls(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.root.join("git.log")).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }

    /// What the stand-in login shell says a word is (`type`'s answer), in
    /// place of what it says of every word by default: `<word> is <a path>`.
    /// `None`: it resolves to nothing.
    pub fn shell_knows(&self, word: &str, resolution: Option<&str>) {
        let line = format!("{word}\t{}\n", resolution.unwrap_or("-"));
        append(&self.root.join("shell-types"), &line);
    }

    /// Takes a command off the PATH afkconvoy runs under, which from here on
    /// holds nothing but `git` and the stand-ins not taken off it.
    pub fn take_off_path(&self, name: &str) {
        let only = self.root.join("path");
        if !only.exists() {
            std::fs::create_dir(&only).unwrap();
            for stand_in in ["gh", "orca"] {
                let built = std::fs::read_link(self.bin.join(stand_in)).unwrap();
                std::os::unix::fs::symlink(built, only.join(stand_in)).unwrap();
            }
            let path = std::env::var_os("PATH").unwrap();
            let git =
                std::env::split_paths(&path).map(|dir| dir.join("git")).find(|git| git.is_file());
            let git = git.expect("git is on PATH");
            write_script(
                &only.join("git"),
                &format!("#!/bin/sh\nexec '{}' \"$@\"\n", git.display()),
            );
        }
        std::fs::remove_file(only.join(name)).unwrap();
    }

    /// Has the stand-in login shell report `tool` (`gh`, `claude`, `pi` or `codex`)
    /// as logged out.
    pub fn logged_out(&self, tool: &str) {
        std::fs::write(self.root.join(format!("logged-out-{tool}")), "").unwrap();
    }

    /// What the stand-in login shell prints when a command is asked for its
    /// `--help`, in place of a help that names every flag Claude Code takes —
    /// and, of Codex's stock command, every flag Codex does.
    /// `None`: the command fails.
    pub fn shell_help(&self, help: Option<&str>) {
        std::fs::write(self.root.join("shell-help"), help.unwrap_or("-")).unwrap();
    }

    /// What the stand-in login shell prints when a command is asked which
    /// models it knows, in place of failing: pi's table of them.
    pub fn shell_models(&self, models: &[&str]) {
        let mut table = "provider      model    context  max-out  thinking  images\n".to_string();
        for model in models {
            let (provider, id) = model.split_once('/').unwrap();
            table.push_str(&format!("{provider}  {id}  272K  128K  yes  yes\n"));
        }
        std::fs::write(self.root.join("shell-models"), table).unwrap();
    }

    /// What the stand-in login shell prints when a command is asked which
    /// models it was built with, in place of failing: Codex's JSON of them.
    pub fn shell_built_with(&self, slugs: &[&str]) {
        let models: Vec<Value> = slugs
            .iter()
            .map(|slug| serde_json::json!({"slug": slug, "display_name": slug, "visibility": "list"}))
            .collect();
        let listed = serde_json::json!({ "models": models }).to_string();
        std::fs::write(self.root.join("shell-built-with"), listed).unwrap();
    }

    /// What the stand-in login shell prints for `alias`.
    pub fn shell_aliases(&self, aliases: &str) {
        std::fs::write(self.root.join("shell-aliases"), aliases).unwrap();
    }

    /// Every script the stand-in login shell was asked to run, in order.
    pub fn shell_asked(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.root.join("shell-asked")).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }

    /// Has the remote turn down every push of a ref whose name starts with
    /// `prefix`, as a server's rule would.
    pub fn remote_rejects(&self, prefix: &str) {
        let hook = self.bare.join("hooks/pre-receive");
        let script = format!(
            "#!/bin/sh\nwhile read old new ref; do\n  case \"$ref\" in {prefix}*) \
             echo 'refs like this are not allowed here' >&2; exit 1;; esac\ndone\n"
        );
        write_script(&hook, &script);
    }

    /// Sets the stand-in GitHub's answer about the base branch's protection:
    /// its rules, or — a string — why they cannot be read.
    pub fn protect(&self, rules: Value) {
        self.with_gh(|gh| gh["protection"][BASE] = rules);
    }

    /// git against the remote itself.
    pub fn remote(&self, args: &[&str]) -> String {
        git(&self.bare, args)
    }
}

/// The copies a fleet kept are read-only, directories included: they are
/// opened again so that the world's directory can be removed.
impl Drop for World {
    fn drop(&mut self) {
        if self.cache().exists() {
            let _ = Command::new("chmod").arg("-R").arg("u+w").arg(self.cache()).status();
        }
    }
}

/// The login shell a worker would get, as far as the fleet asks it anything:
/// `shell -lic 'alias'`, `shell -lic "type -- '<word>'"`, `shell -lic '<command> --help'` —
/// where `pi` answers as pi does, `codex` as Codex and every other command as Claude Code —
/// `shell -lic '… <command> --list-models'`, which fails unless a `shell-models` beside
/// `bin/` holds what it prints, `shell -lic '… <command> debug models --bundled'`, which
/// fails unless a `shell-built-with` there does, and whether `gh`, `claude`, `pi` and `codex` are logged in. Every
/// word is a program unless `shell-types`
/// beside `bin/` says otherwise, and each is logged in unless a
/// `logged-out-<tool>` there says otherwise. It needs nothing of PATH.
const SHELL: &str = r#"#!/bin/sh
root=${0%/*}/..
echo "$2" >> "$root/shell-asked"
case "$2" in
  "gh auth status"*)
    if [ -e "$root/logged-out-gh" ]; then echo "not logged in" >&2; exit 1; fi
    exit 0 ;;
  "claude auth status")
    # Its exit status says nothing either way: the field does.
    if [ -e "$root/logged-out-claude" ]; then echo '{ "loggedIn": false }'; exit 0; fi
    echo 'a line an rc file printed'; echo '{'; echo '  "loggedIn": true'; echo '}'; exit 1 ;;
  "pi --list-models")
    # Its exit status says nothing either way: what it lists does.
    if [ -e "$root/logged-out-pi" ]; then
      echo "No models available. Use /login to log into a provider via OAuth or API key."; exit 0
    fi
    echo "provider      model    context"; echo "openai-codex  gpt-5.5  272K"; exit 0 ;;
  'test -f "$HOME/.codex/auth.json" && codex login status')
    if [ -e "$root/logged-out-codex" ]; then exit 1; fi
    echo "Logged in using ChatGPT"; exit 0 ;;
  alias) cat "$root/shell-aliases" 2>/dev/null; exit 0 ;;
  "codex "*" --help")
    if [ -f "$root/shell-help" ]; then cat "$root/shell-help"; exit 0; fi
    echo "Usage: codex [OPTIONS] [PROMPT]"
    echo "  -c, --config <key=value>"; echo "  -m, --model <MODEL>"
    echo "      --dangerously-bypass-approvals-and-sandbox"
    echo "      --dangerously-bypass-hook-trust"; echo "      --add-dir <DIR>"; exit 0 ;;
  "pi --help")
    echo "Usage:"; echo "  pi [options] [--] [@files...] [messages...]"
    echo "  --model <pattern>              Model pattern or ID"
    echo "  --extension, -e <path>         Load an extension file"
    echo "  --skill <path>                 Load a skill file or directory"
    echo "  --no-skills, -ns               Disable skills discovery and loading"
    echo "  --approve, -a                  Trust project-local files for this run"; exit 0 ;;
  *" --list-models")
    if [ -f "$root/shell-models" ]; then cat "$root/shell-models"; exit 0; fi
    echo "command not found" >&2; exit 127 ;;
  *" debug models --bundled")
    if [ -f "$root/shell-built-with" ]; then cat "$root/shell-built-with"; exit 0; fi
    echo "command not found" >&2; exit 127 ;;
  *" --help")
    if [ -f "$root/shell-help" ]; then
      if [ "$(cat "$root/shell-help")" = "-" ]; then echo "unknown option --help" >&2; exit 1; fi
      cat "$root/shell-help"; exit 0
    fi
    echo "Usage: claude [options]"; echo "  --add-dir <directories...>"
    echo "  --model <model>"; echo "  --setting-sources <sources>"
    echo "  --settings <file-or-json>"; exit 0 ;;
  "type -- "*)
    word=${2#type -- \'}; word=${word%\'}
    if [ -f "$root/shell-types" ]; then
      while IFS='	' read -r name says; do
        if [ "$name" = "$word" ]; then
          if [ "$says" = "-" ]; then echo "$word not found"; exit 1; fi
          echo "$says"; exit 0
        fi
      done < "$root/shell-types"
    fi
    echo "$word is /usr/local/bin/$word"; exit 0 ;;
esac
exit 2
"#;

fn write_script(file: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, script).unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn append(file: &Path, text: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).create(true).open(file).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

fn is_board(body: &str) -> bool {
    body.starts_with("<!--convoy:status-->")
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

/// Commits one file in a checkout → the commit.
pub fn commit(dir: &Path, file: &str, text: &str) -> String {
    write(dir, file, text);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// Writes `text` to `file` under `dir`, making the directories it is in.
pub fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Runs git in `cwd` and returns its trimmed stdout; a failure fails the test.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(cwd).args(args).envs(GIT_ENV).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn read(file: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

/// One rendering of the panel: its markup, to assert on, and its picture, to
/// look at — `None` on a machine with no browser to draw it.
pub struct Rendered {
    pub html: String,
    pub png: Option<PathBuf>,
}

/// The browsers that can draw a page with no window, most wanted first.
/// `CONVOY_BROWSER` names another.
const BROWSERS: [&str; 5] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "chromium",
    "chromium-browser",
    "google-chrome",
];

impl World {
    /// Renders the panel this world's fleet shows — the real `afkconvoy panel`
    /// over the stand-in GitHub and orca — to `<name>.html` and, drawn `width`
    /// by `height`, to `<name>.png`, both under the target directory.
    ///
    /// The page is loaded from the file, so its live script is not found and
    /// nothing in it moves: the same world gives the same picture.
    pub fn render_panel(&self, name: &str, args: &[&str], (width, height): (u32, u32)) -> Rendered {
        let program = env!("CARGO_BIN_EXE_afkconvoy");
        let out = self.command(program, &self.clone).arg("panel").args(args).output().unwrap();
        let html = String::from_utf8(out.stdout).unwrap();
        assert!(out.status.success() && html.starts_with("<!doctype html>"), "{html}");
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("panel");
        std::fs::create_dir_all(&dir).unwrap();
        let (page, png) = (dir.join(format!("{name}.html")), dir.join(format!("{name}.png")));
        std::fs::write(&page, &html).unwrap();
        let _ = std::fs::remove_file(&png);
        let named = std::env::var("CONVOY_BROWSER").ok();
        let drew = named.iter().map(String::as_str).chain(BROWSERS).any(|browser| {
            let browser = Command::new(browser)
                .args(["--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run"])
                .args(["--force-prefers-reduced-motion", "--force-device-scale-factor=2"])
                .arg(format!("--user-data-dir={}", self.root.join("browser").display()))
                .arg(format!("--window-size={width},{height}"))
                .arg(format!("--screenshot={}", png.display()))
                .arg(format!("file://{}", page.display()))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let Ok(mut browser) = browser else { return false };
            // A browser may write its picture and stay: it is waited for only
            // until the picture stops growing, and then put down.
            let mut last = 0;
            for _ in 0..150 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let size = png.metadata().map_or(0, |file| file.len());
                let exited = browser.try_wait().is_ok_and(|status| status.is_some());
                if exited || (size > 0 && size == last) {
                    break;
                }
                last = size;
            }
            let _ = browser.kill();
            let _ = browser.wait();
            png.exists()
        });
        if !drew {
            eprintln!("no browser drew {}: only its markup is checked", page.display());
        }
        Rendered { html, png: drew.then_some(png) }
    }
}

impl Rendered {
    /// The markup of the node `stage` of a lane: its stop, and every card at it.
    pub fn node(&self, stage: &str) -> &str {
        let from = format!("data-stage=\"{stage}\"");
        let after = self.html.split(&from).nth(1).unwrap_or_else(|| panic!("no node {stage}"));
        after.split("<li class=\"node").next().unwrap().split("</ol>").next().unwrap()
    }

    /// The markup of the part of the page that opens with `<tag class="class"`.
    pub fn part(&self, tag: &str, class: &str) -> &str {
        let from = format!("<{tag} class=\"{class}\"");
        let after = self.html.split(&from).nth(1).unwrap_or_else(|| panic!("no {class}"));
        after.split(&format!("</{tag}>")).next().unwrap()
    }
}

/// The issues carded in `markup` on the list `list`, in the order shown.
pub fn carded(markup: &str, list: &str) -> Vec<u64> {
    let Some(cards) = markup.split(&format!("<ul class=\"cards {list}\">")).nth(1) else {
        return vec![];
    };
    let cards = cards.split("</ul>").next().unwrap();
    let numbers = cards.split("data-issue=\"").skip(1);
    numbers.map(|rest| rest.split('"').next().unwrap().parse().unwrap()).collect()
}

/// The card of issue `number` in `markup`, as text: every tag taken out.
pub fn card(markup: &str, number: u64) -> String {
    let from = format!("data-issue=\"{number}\">");
    let after = markup.split(&from).nth(1).unwrap_or_else(|| panic!("no card of #{number}"));
    let mut text = String::new();
    let mut in_tag = false;
    for ch in after.split("</li>").next().unwrap().chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a call of a stand-in asked for: its first two words — `worktree
/// create` — or its one, of a call that has no more: `status`.
pub fn asked(call: &[String]) -> String {
    call.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(" ")
}

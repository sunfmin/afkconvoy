//! The world a test runs afkconvoy in: a bare repo standing in for the remote,
//! a clone of it, and stand-ins for `gh` and `orca` first on PATH.
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

pub const BASE: &str = "main";

/// Opt-in fixture timings, captured by nextest with `--success-output final`.
/// The environment belongs to the test driver, never to a world's host.
pub fn fixture_timing(part: &str, began: Instant) {
    if std::env::var_os("CONVOY_FIXTURE_TIMINGS").is_some() {
        eprintln!("fixture timing: {part} {:.6}s", began.elapsed().as_secs_f64());
    }
}

/// A long-lived fixture command whose children remain attached to it. Keep
/// the root unreaped until drop, and freeze parents before discovering their
/// children so owned pids cannot be reaped/reused while being signalled.
/// Preserve inherited descriptors and the runner's process group: its timeout
/// must still end these processes if the test itself cannot unwind.
pub struct FixtureProcess {
    child: Child,
}

impl FixtureProcess {
    pub fn spawn(command: &mut Command) -> Self {
        Self { child: command.spawn().unwrap() }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    fn state(pid: u32) -> Option<char> {
        let out =
            Command::new("ps").args(["-p", &pid.to_string(), "-o", "stat="]).output().unwrap();
        assert!(
            out.status.success() || (out.status.code() == Some(1) && out.stderr.is_empty()),
            "cannot observe fixture process {pid}"
        );
        String::from_utf8_lossy(&out.stdout).trim().chars().next()
    }

    fn wait_for(pid: u32, stopped: bool) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match Self::state(pid) {
                None | Some('Z') => return,
                Some('T' | 't') if stopped => return,
                _ => {}
            }
            if Instant::now() >= until {
                let message = format!(
                    "fixture process {pid} did not {}",
                    if stopped { "stop" } else { "exit" }
                );
                if std::thread::panicking() {
                    eprintln!("{message}");
                    return;
                }
                panic!("{message}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        let began = Instant::now();
        let root = self.child.id();
        let mut owned = vec![root];
        // SAFETY: this is our unreaped child. Never signal an entire group.
        unsafe { libc::kill(root as libc::pid_t, libc::SIGSTOP) };
        Self::wait_for(root, true);
        loop {
            let listed = Command::new("ps").args(["-axo", "pid=,ppid="]).output().unwrap();
            assert!(listed.status.success(), "cannot discover fixture descendants");
            let children: Vec<u32> = String::from_utf8_lossy(&listed.stdout)
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    let pid = fields.next()?.parse().ok()?;
                    let parent = fields.next()?.parse().ok()?;
                    (owned.contains(&parent) && !owned.contains(&pid)).then_some(pid)
                })
                .collect();
            if children.is_empty() {
                break;
            }
            for pid in children {
                // SAFETY: its frozen parent cannot reap/reuse this child pid.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) };
                Self::wait_for(pid, true);
                owned.push(pid);
            }
        }
        for pid in owned.into_iter().rev() {
            // SAFETY: each parent is still frozen; observe child exit before
            // ending that parent, keeping the child's pid reserved throughout.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            Self::wait_for(pid, false);
        }
        let _ = self.child.wait();
        fixture_timing("owned process teardown", began);
    }
}

/// A regression driver observes inherited leases before allowing teardown.
/// Ordinary scenarios have no rendezvous and return immediately.
pub fn fixture_lease_observed() {
    let Some(root) = std::env::var_os("CONVOY_FIXTURE_LEASE_PROBE") else { return };
    let root = PathBuf::from(root);
    std::fs::write(root.join("ready"), "").unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !root.join("release").exists() {
        assert!(Instant::now() < until, "fixture lease was never observed");
        std::thread::sleep(Duration::from_millis(10));
    }
    if std::env::var("CONVOY_FIXTURE_LEASE_PANIC").as_deref() == Ok("true") {
        panic!("deliberate fixture teardown failure");
    }
}

/// The config every world starts with, committed on the base branch.
pub const CONFIG: &str =
    "```yaml\nbase_branch: main\ngate_command: ./gate.sh\ngate_seconds: 600\n```\n";

/// The expected Codex command in a worktree for selection and continuation
/// tests. `codex.rs` separately checks its decoded arguments and isolation.
pub fn codex_line(_world: &World, dir: &Path) -> String {
    use afkconvoy_core::harness::Harness;
    let notes = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    let home = std::fs::read_to_string(notes.join(Harness::Codex.noted_home().unwrap())).unwrap();
    let model = std::fs::read_to_string(notes.join("convoy-worker-model")).unwrap();
    let command = if model.is_empty() {
        Harness::Codex.stock().to_string()
    } else {
        Harness::Codex.with_model(Harness::Codex.stock(), &model)
    };
    let command = Harness::Codex.in_worktree(&command, &dir.display().to_string(), &home);
    Harness::Codex.guarded(&command, "", "", &terminal_guard())
}

/// A Claude Code launch as an unbound worker receives it, with worktree trust.
pub fn trusted(command: &str) -> String {
    let harness = afkconvoy_core::harness::Harness::ClaudeCode;
    harness.guarded(&harness.in_worktree(command, "", ""), "", "", &terminal_guard())
}

fn terminal_guard() -> String {
    // Each World puts this build on PATH under the name its launch uses.
    "afkconvoy guard --terminal-input-only".to_string()
}

/// The code blocks of `language` the manual shows at `at` — a command as
/// `` `afkconvoy config` ``, a topic as `` topic `example` `` — each as its
/// reader sees it: a test runs what the help shows, so the two cannot drift.
pub fn shown(at: &str, language: &str) -> Vec<String> {
    let blocks = agent_help::code_blocks(&afkconvoy::manual(), language).into_iter();
    let blocks: Vec<String> = blocks.filter(|block| block.at == at).map(|b| b.code).collect();
    assert!(!blocks.is_empty(), "{at} shows no {language} block");
    blocks
}

pub struct World {
    _root: TempDir,
    tmp_links: TempDir,
    pub root: PathBuf,
    /// The remote: every clone's `origin`.
    pub bare: PathBuf,
    /// The checkout the fleet runs in.
    pub clone: PathBuf,
    bin: PathBuf,
    gh_file: PathBuf,
    orca_file: PathBuf,
    repository_templates: std::sync::atomic::AtomicBool,
    termination_clock: std::sync::atomic::AtomicBool,
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
    /// A repo with a remote, the stand-ins on PATH, and a host kit that holds
    /// Claude Code alone, on no model: it leaves no choice to make, so no
    /// caster is started (`hold_every_harness` holds the others).
    pub fn new() -> World {
        let began = Instant::now();
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (bare, clone, bin) = (root.join("remote.git"), root.join("clone"), root.join("bin"));
        git(&root, &["init", "-q", "--bare", "-b", BASE, bare.to_str().unwrap()]);
        // git takes a pusher's config off before it starts the remote's side:
        // the remote is told by itself to do no housekeeping (`GIT_ENV`).
        append(&bare.join("config"), "[maintenance]\n\tauto = false\n");
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
        write_script(&bin.join("curl"), CURL);
        // The host a fleet reads: nothing of the one the tests run on. `pmset`
        // stands in so that nothing a test runs can put that one to sleep.
        for (name, script) in [("pmset", PMSET), ("sysctl", SYSCTL), ("df", DF)] {
            write_script(&bin.join(name), script);
        }
        // The machine is this account's alone, and its `afkconvoy` this build.
        write_script(&bin.join("ps"), PS);
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_afkconvoy"), bin.join("afkconvoy")).unwrap();
        let world = World {
            repository_templates: std::sync::atomic::AtomicBool::new(false),
            termination_clock: std::sync::atomic::AtomicBool::new(false),
            tmp_links: tempfile::tempdir_in("/tmp").unwrap(),
            gh_file: root.join("gh.json"),
            orca_file: root.join("orca.json"),
            _root: dir,
            root,
            bare,
            clone,
            bin,
        };
        world.write_gh(&json!({"bare": world.bare, "base": BASE, "issues": [], "prs": [],
            "comments": {}, "labels": ["ready-for-agent"], "calls": [], "envs": []}));
        world.write_orca(&json!({"repos": [{"id": "repo-1", "path": world.clone}],
            "root": world.root.join("wt"), "worktrees": [], "terminals": [], "calls": [],
            "envs": []}));
        world.kit_command("claude-code", "agent");
        // Claims in this world normally call its local host h1. Commands
        // without a --host flag must read that same identity from settings.
        world.say_host_settings(json!({"host_name": "h1"}));
        // What a fresh login shell of its host user starts with: its
        // commands' environment, a name only it sets, and a line its rc
        // files print first.
        let exported: String = world
            .login_env()
            .iter()
            .map(|(name, value)| format!("export {name}='{}'\n", value.replace('\'', "'\\''")))
            .collect();
        let profile = "export AFKCONVOY_TEST_LOGIN=1\necho 'a line an rc file printed'\n";
        write(&world.home(), ".profile", &format!("{exported}{profile}"));
        fixture_timing("world setup", began);
        world
    }

    /// Has the host kit hold every harness after Claude Code, each on its stock
    /// command: a choice to make.
    pub fn hold_every_harness(&self) {
        self.kit_command("pi", afkconvoy_core::harness::Harness::Pi.stock());
        self.kit_command("codex", afkconvoy_core::harness::Harness::Codex.stock());
        self.kit_command("dsh", afkconvoy_core::harness::Harness::Dsh.stock());
        self.kit_command("kimi", afkconvoy_core::harness::Harness::Kimi.stock());
        self.kit_command("grok", afkconvoy_core::harness::Harness::Grok.stock());
    }

    /// Has the host kit hold `models` for `harness`, in that order, as seen
    /// to answer.
    pub fn kit_models(&self, harness: &str, models: &[&str]) {
        let models: Vec<Value> = models.iter().map(|model| json!({"model": model})).collect();
        self.with_kit(|kit| {
            let held = kit["harnesses"].as_array_mut().unwrap();
            let held = held.iter_mut().find(|held| held["harness"] == harness).unwrap();
            held["models"] = json!(models);
        });
    }

    /// Holds dated model research for scenarios about workflows and worker
    /// starts, so pending research does not start an unrelated host watch.
    pub fn kit_researched_models(&self, harness: &str, models: &[&str]) {
        self.kit_models(harness, models);
        self.with_kit(|kit| {
            let held = kit["harnesses"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|held| held["harness"] == harness)
                .unwrap();
            for model in held["models"].as_array_mut().unwrap() {
                model["good_at"] = json!("coding and review");
                model["sources"] = json!([
                    {"url":"https://vendor.example/model-card", "date":"2026-10-24"}
                ]);
                model["researched_at"] = json!(900);
            }
        });
    }

    /// Changes the host kit in place.
    pub fn with_kit(&self, change: impl FnOnce(&mut Value)) {
        let mut kit: Value =
            serde_json::from_slice(&std::fs::read(self.kit_path()).unwrap()).unwrap();
        change(&mut kit);
        std::fs::write(self.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    }

    /// What the host kit's first answers a caster's prompt with.
    pub fn caster_answers(&self, answer: &str) {
        std::fs::write(self.root.join("caster-answer"), answer).unwrap();
    }

    /// Has the model a roster's draft is asked of answer `answer`.
    pub fn drafter_answers(&self, answer: &str) {
        std::fs::write(self.root.join("drafter-answer"), answer).unwrap();
    }

    /// The prompt the model a roster's draft is asked of was sent last.
    pub fn drafter_asked(&self) -> String {
        std::fs::read_to_string(self.root.join("drafter-asked")).unwrap_or_default()
    }

    /// Has every model's probe answered from here on.
    pub fn probes_answer(&self) {
        std::fs::write(self.root.join("probes-answer"), "").unwrap();
    }

    /// Where this host keeps the harnesses paused on it.
    pub fn paused_path(&self) -> PathBuf {
        self.cache().join("afkconvoy/paused.json")
    }

    /// The harnesses paused on this host, as it keeps them.
    pub fn paused(&self) -> Value {
        std::fs::read(self.paused_path())
            .map_or(json!([]), |bytes| serde_json::from_slice(&bytes).unwrap())
    }

    /// Pauses `harness` on this host since `since`, for `why`, as a fleet
    /// that found it refusing every prompt does.
    pub fn pause(&self, harness: &str, since: u64, why: &str) {
        let mut paused = self.paused();
        paused
            .as_array_mut()
            .unwrap()
            .push(json!({"harness": harness, "since": since, "why": why}));
        std::fs::create_dir_all(self.paused_path().parent().unwrap()).unwrap();
        std::fs::write(self.paused_path(), serde_json::to_vec(&paused).unwrap()).unwrap();
    }

    /// The last prompt a caster sent; empty when none was.
    pub fn caster_asked(&self) -> String {
        std::fs::read_to_string(self.root.join("caster-asked")).unwrap_or_default()
    }

    /// A command with the stand-ins first on PATH and nothing of the caller's
    /// git or GitHub setup leaking in.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>, cwd: &Path) -> Command {
        let mut cmd = Command::new(program);
        for name in [
            "AFKCONVOY_WATCH",
            "AFKCONVOY_DSH_MODEL",
            "DSH_HOME",
            "DSH_AGENTS_HOME",
            "DSH_PROFILE",
            "DSH_PROFILE_DIR",
            // The share the gate running these tests was given is not the
            // share of a gate in the world (sunfmin/control#245).
            "CARGO_BUILD_JOBS",
            "NEXTEST_TEST_THREADS",
        ] {
            cmd.env_remove(name);
        }
        cmd.current_dir(cwd)
            .envs(self.login_env())
            .env("SHELL", self.bin.join("shell"))
            .env("XDG_CACHE_HOME", self.cache())
            .env_remove("XDG_CONFIG_HOME")
            .env("HOME", self.home())
            .env_remove("AFKCONVOY_ISSUE")
            .env_remove("ANTHROPIC_BASE_URL");
        // A stand-in world never inherits the host's real GitHub credential.
        // Quota identity scenarios opt into an explicit fixture token.
        for name in ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"] {
            cmd.env_remove(name);
        }
        cmd
    }

    /// What every command of this world is run with beyond the caller's
    /// environment: the stand-ins first on PATH and nothing of the caller's git
    /// or GitHub setup. The same is the world's login profile (`home/.profile`,
    /// written once, with the world): what a fresh login shell of its host user
    /// starts with, and so a host runner too.
    pub fn login_env(&self) -> Vec<(String, String)> {
        let only = self.root.join("path");
        let path = match only.exists() {
            true => only.display().to_string(),
            false => match &*REAL_GIT_DIR {
                Some(dir) => format!(
                    "{}:{}:{}",
                    self.bin.display(),
                    dir.display(),
                    std::env::var("PATH").unwrap()
                ),
                None => format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            },
        };
        let shown = |path: &Path| path.display().to_string();
        let mut env = vec![
            ("PATH".to_string(), path),
            ("AFKCONVOY_TEST_OWNER".to_string(), std::process::id().to_string()),
            ("CONVOY_FAKE_GH".to_string(), shown(&self.gh_file)),
            ("CONVOY_FAKE_ORCA".to_string(), shown(&self.orca_file)),
            ("AFKCONVOY_TMP_LINKS".to_string(), shown(&self.tmp_links.path().join("links"))),
        ];
        env.extend(GIT_ENV.iter().map(|(name, value)| (name.to_string(), value.to_string())));
        // No checkout above the world is one of its: a test's directory
        // is in this repo's, when a worker's TMPDIR is in its worktree.
        env.push(("GIT_CEILING_DIRECTORIES".to_string(), shown(self.root.parent().unwrap())));
        // The fleet polls a session file for the turn. The stand-in has
        // already written it when send returns, so a test checks once.
        env.push(("AFKCONVOY_TURN_WAIT_SECONDS".to_string(), "0".to_string()));
        // The same for a note typed at a worker (`afkconvoy tell`).
        env.push(("AFKCONVOY_NOTE_WAIT_SECONDS".to_string(), "0".to_string()));
        env
    }

    /// Where this world's fleet keeps what it caches on its host: the copies
    /// of a team and its roles are under it.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// This world's worker link for `worktree`, with the production key but
    /// in its own short directory, never among the host's real links.
    pub fn tmp(&self, worktree: &Path) -> PathBuf {
        let link = afkconvoy::worker_tmp::of(worktree);
        self.tmp_links.path().join("links").join(link.file_name().unwrap())
    }

    /// The home of this world's host user: nothing of the home of whoever
    /// runs the tests is read. It holds nothing but what a test puts there.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub fn kit_path(&self) -> PathBuf {
        self.home().join(".config/afkconvoy/kit.json")
    }

    /// Set a host command directly, as the terminal editor would save it.
    pub fn kit_command(&self, harness: &str, command: &str) {
        let file = self.kit_path();
        let mut kit = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or_else(|| json!({"harnesses": []}));
        let held = kit["harnesses"].as_array_mut().unwrap();
        match held.iter_mut().find(|held| held["harness"] == harness) {
            Some(held) => held["command"] = json!(command),
            None => held.push(json!({"harness": harness, "command": command})),
        }
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, serde_json::to_vec(&kit).unwrap()).unwrap();
    }

    /// Set the Jev key command directly in the host's checkout-list settings.
    pub fn jev_key_command(&self, command: &str) {
        let file = self.cache().join("afkconvoy/checkouts.json");
        let mut listed = self.checkout_list();
        listed["settings"]["jev_key_command"] = json!(command);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, serde_json::to_vec(&listed).unwrap()).unwrap();
    }

    /// Has the host name a Jev key command, and Jev answer the next check: no
    /// to every judgment but those `said`, and `evidence` selected from the
    /// screen.
    pub fn jev_judges(&self, said: &[(&str, f64)], evidence: &str) {
        self.jev_key_command("printf test-key");
        let mut answers = serde_json::Map::new();
        for id in afkconvoy_core::worker_health::judgments() {
            let noul = said.iter().find(|(said, _)| *said == id).map_or(0.01, |(_, noul)| *noul);
            answers.insert(id.into(), json!({"type": "noul", "noul": noul}));
        }
        answers.insert(
            "evidence".into(),
            json!({"type": "choice", "choice": evidence, "confidence": 0.99,
                "probabilities": {evidence: 0.99}}),
        );
        self.jev_answers(Value::Object(answers));
    }

    /// Give the TypeSafe stand-in one response body, with these answers.
    pub fn jev_answers(&self, answers: Value) {
        self.typesafe_answers(&json!({"model": "jev-recorded", "answers": answers,
            "usage": {"input_tokens": 100, "output_tokens": 7}}));
    }

    /// Give the TypeSafe stand-in what Jev answered for the real screen
    /// recorded as `screen` → the lines of that screen, and the response.
    pub fn jev_recorded(&self, screen: &str) -> (Vec<String>, Value) {
        let recorded: Value =
            serde_json::from_str(include_str!("../../../core/src/worker_health/recorded.json"))
                .unwrap();
        let case = recorded["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["screen"] == screen)
            .unwrap_or_else(|| panic!("no recorded screen `{screen}`"));
        // The historical recording predates the short question. Its earlier
        // settled answer decides the class; this extra answer is test data.
        let mut response = case["response"].clone();
        response["answers"]["short"] = json!({"type": "noul", "noul": 0.1});
        self.typesafe_answers(&response);
        let lines = case["lines"].as_array().unwrap();
        (lines.iter().map(|line| line.as_str().unwrap().to_string()).collect(), response)
    }

    /// Give the TypeSafe stand-in this whole response body.
    pub fn typesafe_answers(&self, response: &Value) {
        std::fs::write(self.root.join("typesafe-answer"), response.to_string()).unwrap();
    }

    /// What the TypeSafe stand-in was last sent: curl's arguments, and the
    /// config it read on its standard input.
    pub fn typesafe_sent(&self) -> (String, String) {
        let sent = std::fs::read_to_string(self.root.join("typesafe-request")).unwrap();
        let (arguments, config) = sent.split_once('\n').unwrap();
        (arguments.to_string(), config.to_string())
    }

    /// The request body the TypeSafe stand-in was last sent, unquoted from
    /// curl's config as curl reads it.
    pub fn typesafe_payload(&self) -> Value {
        let (_, config) = self.typesafe_sent();
        let quoted = config
            .lines()
            .find_map(|line| line.strip_prefix("data-binary = "))
            .expect("the payload is in the config");
        let mut body = String::new();
        let mut chars = quoted.trim_matches('"').chars();
        while let Some(c) = chars.next() {
            body.push(if c == '\\' { chars.next().unwrap() } else { c });
        }
        serde_json::from_str(&body).unwrap()
    }

    /// Runs the real CLI in the fleet's checkout.
    pub fn afk(&self, args: &[&str]) -> Answer {
        self.afk_in(&self.clone, args)
    }

    pub fn afk_in(&self, cwd: &Path, args: &[&str]) -> Answer {
        let program = Path::new(env!("CARGO_BIN_EXE_afkconvoy"));
        if self.termination_clock.load(std::sync::atomic::Ordering::Relaxed) {
            self.afk_as(
                &program.parent().unwrap().join("examples/test_termination_clock"),
                cwd,
                args,
            )
        } else {
            self.afk_as(program, cwd, args)
        }
    }

    /// Advance only termination time in the real CLI entry point. Use only
    /// when Orca's terminal is a stand-in that deliberately never closes.
    pub fn control_termination_time(&self) {
        self.termination_clock.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Reuse immutable Git seeds for subsequent role/team repositories. Each
    /// call to `repo` still gets independent files, objects, refs and index.
    pub fn reuse_repository_templates(&self) {
        self.repository_templates.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// `afk_in`, as the afkconvoy at `program`: one installed elsewhere.
    pub fn afk_as(&self, program: &Path, cwd: &Path, args: &[&str]) -> Answer {
        let out = self.command(program, cwd).args(self.settled_args(cwd, args)).output().unwrap();
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

    /// Running-fleet tests choose their own private repo on a first start
    /// unless they name another tracker. Existing choices stand. Tests of
    /// unanswered first starts use `command` directly, without this choice.
    fn settled_args<'a>(&self, cwd: &Path, args: &[&'a str]) -> Vec<&'a str> {
        let mut args = args.to_vec();
        let listed = self.checkout_list();
        let known = listed["checkouts"].as_array().unwrap().iter().any(|entry| {
            entry["checkout"].as_str().is_some_and(|checkout| cwd.starts_with(checkout))
        });
        if args.first() == Some(&"run")
            && !known
            && !args.iter().any(|arg| *arg == "--tracker" || arg.starts_with("--tracker="))
        {
            args.extend(["--tracker", "default"]);
        }
        args
    }

    /// Sets the repo's gate command: committed on the base branch and pushed,
    /// as a repo's config is.
    pub fn set_gate(&self, command: &str) {
        self.set_gate_config(&format!("gate_command: {command:?}\ngate_seconds: 600"));
    }

    /// Commits a config of the gate's keys as `yaml` has them, and pushes it.
    pub fn set_gate_config(&self, yaml: &str) {
        let config = format!("```yaml\nbase_branch: main\n{yaml}\n```\n");
        std::fs::write(self.clone.join("docs/agents/afkconvoy.md"), config).unwrap();
        git(&self.clone, &["commit", "-q", "-am", "set the gate"]);
        git(&self.clone, &["push", "-q", "origin", BASE]);
    }

    /// Runs a command that prints lines before its answer → (the lines, the
    /// answer), but for the lines a `run` says of itself whatever it does: its
    /// start, each tick's, and the host's independent watch (`bookkeeping`).
    pub fn afk_lines(&self, args: &[&str]) -> (Vec<String>, Value) {
        let (lines, answer) = self.afk_every_line(args);
        (lines.into_iter().filter(|line| !bookkeeping(line)).collect(), answer)
    }

    /// Runs a command that prints lines before its answer → (every line, the answer).
    pub fn afk_every_line(&self, args: &[&str]) -> (Vec<String>, Value) {
        let out = self
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &self.clone)
            .args(self.settled_args(&self.clone, args))
            .output()
            .unwrap();
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

    /// Changes GitHub state under the stand-in's transaction lock. A tick
    /// or watch look may otherwise overwrite an issue the test just added.
    pub fn with_gh(&self, change: impl FnOnce(&mut Value)) {
        let lock = std::fs::File::create(self.gh_file.with_extension("lock")).unwrap();
        lock.lock().unwrap();
        let mut state = self.gh();
        change(&mut state);
        self.write_gh(&state);
    }

    /// Puts another repo on the stand-in GitHub as `name` (`owner/repo`), with
    /// `files` committed on its default branch → its checkout, to commit more to.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let began = Instant::now();
        let dir = self.root.join("github").join(name);
        if self.repository_templates.load(std::sync::atomic::Ordering::Relaxed) {
            copy_repository(&repository_template(files), &dir);
        } else {
            create_repository(&dir, files);
        }
        self.with_gh(|gh| gh["repos"][name] = json!(dir));
        fixture_timing("repo setup", began);
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
        self.pr_closing(number, &format!("#{closes}"), branch);
    }

    /// Adds an open PR from `branch` whose body closes the issue the repo
    /// names `issue`: `#7` of its own, `acme/tracker#7` of another repo's.
    pub fn pr_closing(&self, number: u64, issue: &str, branch: &str) {
        self.with_gh(|gh| {
            let pr = json!({"number": number, "title": format!("pr {number}"), "headRefName": branch, "headRefOid": "",
                "body": format!("Closes {issue}\n"), "state": "open"});
            gh["prs"].as_array_mut().unwrap().push(pr);
        });
    }

    /// Adds an open PR from a fork's `branch` whose body closes issue
    /// `closes`: anyone's, its head in another repository.
    pub fn fork_pr(&self, number: u64, closes: u64, branch: &str) {
        self.pr(number, closes, branch);
        self.with_gh(|gh| {
            gh["prs"].as_array_mut().unwrap().last_mut().unwrap()["fork"] = json!(true)
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
            // A worker's verdict is posted after its start. Tests that need a
            // different or missing time set it explicitly after posting.
            let at = if body.starts_with("<!--convoy:verdict ") {
                rows.as_array().unwrap().iter().rev().find_map(|row| {
                    let fields = afkconvoy_core::record::read_marker(
                        afkconvoy_core::account::WORKER,
                        row["body"].as_str()?,
                    )?;
                    fields.get("since")?.parse::<u64>().ok()
                })
            } else {
                None
            };
            rows.as_array_mut()
                .unwrap()
                .push(json!({"id": id, "body": body, "html_url": url, "at": at}));
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
    /// reads every start makes: of the base branch's protection, and of who
    /// may see the tracker.
    pub fn gh_calls(&self) -> Vec<Vec<String>> {
        let mut calls: Vec<Vec<String>> =
            serde_json::from_value(self.gh()["calls"].clone()).unwrap();
        let visibility = afkconvoy::gh::VISIBILITY_JQ;
        calls.retain(|call| {
            !call.iter().any(|arg| arg.ends_with("/protection") || arg == visibility)
        });
        self.with_gh(|gh| {
            gh["calls"] = json!([]);
            gh["envs"] = json!([]);
        });
        calls
    }

    /// Every `gh` call made, as its argv, with the names of a session's
    /// environment it ran in (`envs`).
    pub fn gh_called_with(&self) -> Vec<(Vec<String>, Vec<String>)> {
        called_with(&self.gh())
    }

    /// Every `orca` call made, as its argv, with the names of a session's
    /// environment it ran in (`envs`).
    pub fn orca_called_with(&self) -> Vec<(Vec<String>, Vec<String>)> {
        called_with(&self.orca())
    }

    pub fn write_gh(&self, state: &Value) {
        std::fs::write(&self.gh_file, state.to_string()).unwrap();
    }

    pub fn orca(&self) -> Value {
        read(&self.orca_file)
    }

    /// Changes the stand-in orca's state under the same lock its calls hold:
    /// a running host runner may be calling it while a test changes it.
    pub fn with_orca(&self, change: impl FnOnce(&mut Value)) {
        let lock = std::fs::File::create(self.orca_file.with_extension("lock")).unwrap();
        lock.lock().unwrap();
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

    /// Each question put to the stand-in orca's login shell in a terminal of
    /// its own, in order: what it asked, and whether its terminal is open.
    pub fn questions(&self) -> Vec<(String, bool)> {
        let orca = self.orca();
        let rows = orca["questions"].as_array().cloned().unwrap_or_default();
        rows.iter()
            .map(|row| (row["script"].as_str().unwrap().to_string(), row["open"] == true))
            .collect()
    }

    /// Every `orca` call made since the last look, without the trailing `--json`.
    pub fn orca_calls(&self) -> Vec<Vec<String>> {
        let mut calls: Vec<Vec<String>> =
            serde_json::from_value(self.orca()["calls"].clone()).unwrap();
        for call in &mut calls {
            call.pop();
        }
        self.with_orca(|orca| {
            orca["calls"] = json!([]);
            orca["envs"] = json!([]);
            orca["parents"] = json!([]);
        });
        calls
    }

    /// The bodies of the comments on an issue or a PR, oldest first — all but
    /// its status board, which is `board`'s.
    pub fn comments(&self, number: u64) -> Vec<String> {
        self.every_comment(number).into_iter().filter(|body| !is_board(body)).collect()
    }

    /// The URL GitHub assigned to this comment, to assert a wait links to
    /// the explanation rather than to its issue or a later status comment.
    pub fn comment_url(&self, number: u64, body: &str) -> String {
        let gh = self.gh();
        let rows = gh["comments"][number.to_string()].as_array().unwrap();
        let comment = rows.iter().rev().find(|row| row["body"] == body).unwrap();
        comment["html_url"].as_str().unwrap().to_string()
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
        let (git, log) = (git_program(), self.root.join("git.log"));
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            git.display()
        );
        write_script(&self.bin.join("git"), &script);
    }

    /// Makes the first `stalls` listings of the remote sleep past the short
    /// limit the test gives afkconvoy, and logs every git call.
    pub fn stall_git_lists(&self, stalls: usize) {
        let (git, log, calls) =
            (git_program(), self.root.join("git.log"), self.root.join("git-list-calls"));
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{0}'\nif [ \"$1\" = ls-remote ]; then\n  n=0\n  if [ -f '{2}' ]; then n=$(cat '{2}'); fi\n  n=$((n + 1))\n  echo \"$n\" > '{2}'\n  if [ \"$n\" -le {1} ]; then sleep 60; fi\nfi\nexec '{3}' \"$@\"\n",
            log.display(),
            stalls,
            calls.display(),
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
            let git = git_program();
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

    /// Has the stand-in harness print `first` to its first login question and
    /// `second` to its second; both answers exit zero.
    pub fn login_answers(&self, tool: &str, first: &str, second: &str) {
        std::fs::write(self.root.join(format!("login-first-{tool}")), first).unwrap();
        std::fs::write(self.root.join(format!("login-second-{tool}")), second).unwrap();
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

    /// What the stand-in login shell prints when Claude Code is asked which
    /// models its login runs, in place of failing: its answer to `initialize`.
    pub fn shell_claude_models(&self, models: &[&str]) {
        let models: Vec<Value> = models.iter().map(|model| json!({"value": model})).collect();
        let answer = json!({"type": "control_response",
            "response": {"subtype": "success", "response": {"models": models}}});
        std::fs::write(self.root.join("shell-claude-models"), answer.to_string()).unwrap();
    }

    /// What the stand-in login shell prints when a command is asked which
    /// models its server lists, in place of failing: Codex's JSON of them.
    pub fn shell_server_models(&self, slugs: &[&str]) {
        let models: Vec<Value> = slugs
            .iter()
            .map(|slug| serde_json::json!({"slug": slug, "display_name": slug, "visibility": "list"}))
            .collect();
        let listed = serde_json::json!({ "models": models }).to_string();
        std::fs::write(self.root.join("shell-server-models"), listed).unwrap();
    }

    /// What the stand-in login shell prints for `alias`.
    pub fn shell_aliases(&self, aliases: &str) {
        std::fs::write(self.root.join("shell-aliases"), aliases).unwrap();
    }

    /// Whether a question carrying `line` was put to a harness, in a terminal
    /// of its own: what the harness diagnosis of a worker's screen did.
    pub fn asked_a_harness_about(&self, line: &str) -> bool {
        let calls: Vec<Vec<String>> =
            serde_json::from_value(self.orca()["calls"].clone()).unwrap_or_default();
        calls.iter().any(|call| {
            call.iter().any(|arg| arg == "afkconvoy question")
                && call.iter().any(|arg| arg.contains(line))
        })
    }

    /// Every script the stand-in login shell was asked to run, in order.
    pub fn shell_asked(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.root.join("shell-asked")).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }

    /// The directory the stand-in login shell was started in, each time it
    /// was, in order.
    pub fn shell_asked_in(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.root.join("shell-asked-in")).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }

    /// The directory the stand-in orca was run from, each time it was, in
    /// order: real orca takes the worktree a call names none of from there.
    pub fn orca_run_from(&self) -> Vec<String> {
        serde_json::from_value(self.orca()["from"].clone()).unwrap_or_default()
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

fn create_repository(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", BASE]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    for (file, text) in files {
        write(dir, file, text);
    }
    if !files.is_empty() {
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "files"]);
    }
}

/// Built privately, then published by one atomic rename. Readers see only a
/// complete, read-only seed. Concurrent builders discard their own copies.
/// The manifest checks the full input even if two inputs hash to one name.
pub(crate) fn repository_template(files: &[(&str, &str)]) -> PathBuf {
    // Cargo keeps a test binary's filename across source edits. Changing the
    // builder or GIT_ENV must rebuild seeds too, without a manual cache bump.
    publish_repository_template(files, include_str!("mod.rs"))
}

/// `repository_template`, hashing `builder` in place of this file. A test
/// passes another source to prove a change publishes a new seed.
pub(crate) fn publish_repository_template(files: &[(&str, &str)], builder: &str) -> PathBuf {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::os::unix::fs::PermissionsExt;
    let manifest = serde_json::to_vec(&(BASE, files)).unwrap();
    let mut hash = DefaultHasher::new();
    manifest.hash(&mut hash);
    builder.hash(&mut hash);
    let templates = repository_templates_dir();
    std::fs::create_dir_all(&templates).unwrap();
    // `chmod` of a seed stops at that seed. The directory holding them stays
    // writable, so a later publisher can add one.
    let held = std::fs::metadata(&templates).unwrap().permissions().mode();
    assert_ne!(held & 0o200, 0, "the repository template directory is not writable by its owner");
    let kept = templates.join(format!("{:016x}", hash.finish()));
    if !kept.exists() {
        let scratch = tempfile::tempdir_in(&templates).unwrap();
        create_repository(&scratch.path().join("repo"), files);
        std::fs::write(scratch.path().join("manifest.json"), &manifest).unwrap();
        publish_scratch(scratch.path(), &kept);
    }
    assert_eq!(std::fs::read(kept.join("manifest.json")).unwrap(), manifest);
    kept.join("repo")
}

/// Freezes `scratch` and publishes it as `kept` by one rename. A publisher
/// that loses the race thaws its own scratch so the scratch can be removed.
pub(crate) fn publish_scratch(scratch: &Path, kept: &Path) {
    let frozen = Command::new("chmod").args(["-R", "a-w"]).arg(scratch).status().unwrap();
    assert!(frozen.success(), "freezing the repository template");
    if let Err(error) = std::fs::rename(scratch, kept) {
        // Another complete seed may have won. Open our scratch again so it
        // can be removed, including when the failure was real.
        let thawed = Command::new("chmod").args(["-R", "u+w"]).arg(scratch).status().unwrap();
        assert!(thawed.success(), "thawing the losing repository template");
        assert!(kept.is_dir(), "cannot publish repository template: {error}");
    }
}

/// Where repository template seeds are kept.
///
/// The process temp directory, when that is outside the checkout, the test
/// binary's build output and the fleet's cache directory. The gate points
/// `TMPDIR` at a directory inside the checkout, and a read-only seed there
/// stops `git worktree remove` with `Permission denied`. `/tmp` is outside
/// the checkout. Nothing here reads or writes the fleet's cache directory:
/// its path is only a place the seed must not be.
fn repository_templates_dir() -> PathBuf {
    let checkout = std::env::current_exe().ok().as_deref().and_then(enclosing_checkout);
    let fleet_cache = afkconvoy::copies::cache().ok();
    repository_template_home(
        &std::env::temp_dir(),
        checkout.as_deref(),
        build_output().as_deref(),
        fleet_cache.as_deref(),
    )
}

/// The directory `repository_templates_dir` chooses for these roots.
pub(crate) fn repository_template_home(
    process_temp: &Path,
    checkout: Option<&Path>,
    build: Option<&Path>,
    fleet_cache: Option<&Path>,
) -> PathBuf {
    let blocked = lies_inside(process_temp, &[checkout, build, fleet_cache]);
    let base = if blocked { PathBuf::from("/tmp") } else { process_temp.to_path_buf() };
    base.join("afkconvoy-repository-templates-v1")
}

/// Whether `dir` is `root` or a path under it. A missing path is compared as
/// given, so a cache directory that is not there yet still counts.
fn lies_inside(dir: &Path, roots: &[Option<&Path>]) -> bool {
    let dir = canonical(dir);
    roots.iter().flatten().any(|root| dir.starts_with(canonical(root)))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The checkout that holds `path`: the directory whose `.git` is its own,
/// walking up. A worktree's `.git` is a file, and that still marks it.
fn enclosing_checkout(path: &Path) -> Option<PathBuf> {
    let mut dir = canonical(path);
    if !dir.is_dir() && !dir.pop() {
        return None;
    }
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The cargo target directory of this test binary: the parent of the
/// `debug` or `release` directory the binary sits under, whether cargo put
/// it in `deps` or under the build script's output.
fn build_output() -> Option<PathBuf> {
    let exe = canonical(&std::env::current_exe().ok()?);
    let mut dir = exe.parent()?.to_path_buf();
    loop {
        let profile = dir.file_name().is_some_and(|name| name == "debug" || name == "release");
        if profile {
            return dir.parent().map(Path::to_path_buf);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Ordinary copies, never hard links or Git alternates: committing, changing
/// permissions or removing a world's repo cannot change the seed or a peer.
fn copy_repository(from: &Path, to: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source).unwrap();
        if metadata.is_dir() {
            copy_repository(&source, &target);
        } else if metadata.file_type().is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&source).unwrap(), target).unwrap();
        } else {
            std::fs::copy(&source, &target).unwrap();
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o200);
            std::fs::set_permissions(target, permissions).unwrap();
        }
    }
}

/// A host runner this world started ends with it: killed, so that none is
/// left ticking against a world that is gone. The copies a fleet kept are
/// read-only, directories included: they are opened again so that the world's
/// directory can be removed.
impl Drop for World {
    fn drop(&mut self) {
        let until = Instant::now() + Duration::from_secs(5);
        while self.runner_lock_held() && Instant::now() < until {
            if let Some(pid) = self.host_runner() {
                let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.cache().exists() {
            let _ = Command::new("chmod").arg("-R").arg("u+w").arg(self.cache()).status();
        }
    }
}

/// The calls of a stand-in's `state`, each with the names of a session's
/// environment it ran in.
fn called_with(state: &Value) -> Vec<(Vec<String>, Vec<String>)> {
    let calls: Vec<Vec<String>> = serde_json::from_value(state["calls"].clone()).unwrap();
    let envs: Vec<Vec<String>> = serde_json::from_value(state["envs"].clone()).unwrap_or_default();
    assert_eq!(calls.len(), envs.len(), "an environment beside each call");
    calls.into_iter().zip(envs).collect()
}

impl World {
    /// A second repo on this world's host: a remote of its own
    /// (`remote2.git`), a clone of it with the config committed (`clone2`),
    /// which orca is given too, and a stand-in GitHub of its own (`gh2`),
    /// which every `gh` run in that clone reaches → the clone.
    pub fn second_repo(&self) -> PathBuf {
        let (bare, clone) = (self.root.join("remote2.git"), self.root.join("clone2"));
        git(&self.root, &["init", "-q", "--bare", "-b", BASE, bare.to_str().unwrap()]);
        append(&bare.join("config"), "[maintenance]\n\tauto = false\n");
        git(&self.root, &["clone", "-q", bare.to_str().unwrap(), clone.to_str().unwrap()]);
        std::fs::write(clone.join("README.md"), "gadgets\n").unwrap();
        write(&clone, "docs/agents/afkconvoy.md", CONFIG);
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-q", "-m", "initial"]);
        git(&clone, &["push", "-q", "origin", BASE]);
        let gh2 = self.root.join("gh2.json");
        let state = json!({"bare": bare, "base": BASE, "issues": [], "prs": [], "comments": {},
            "labels": ["ready-for-agent"], "calls": [], "envs": []});
        std::fs::write(&gh2, state.to_string()).unwrap();
        std::fs::write(clone.join(".git/convoy-fake-gh"), gh2.display().to_string()).unwrap();
        self.with_orca(|orca| {
            let repos = orca["repos"].as_array_mut().unwrap();
            repos.push(json!({"id": "repo-2", "path": clone}));
        });
        clone
    }

    /// The stand-in GitHub of the second repo (`second_repo`).
    pub fn gh2(&self) -> Value {
        read(&self.root.join("gh2.json"))
    }

    /// Another clone of the remote, as `name` in the world → its checkout.
    pub fn clone_again(&self, name: &str) -> PathBuf {
        let again = self.root.join(name);
        git(&self.root, &["clone", "-q", self.bare.to_str().unwrap(), again.to_str().unwrap()]);
        again
    }

    /// This host's checkout list, as the host runner keeps it: none yet is
    /// an empty one.
    pub fn checkout_list(&self) -> Value {
        let file = self.cache().join("afkconvoy/checkouts.json");
        std::fs::read(file)
            .map_or(json!({"checkouts": []}), |bytes| serde_json::from_slice(&bytes).unwrap())
    }

    /// Has this world's host say `settings`, as a `run` that named them would
    /// have it: its host settings, kept beside the checkout list.
    pub fn say_host_settings(&self, settings: Value) {
        let file = self.cache().join("afkconvoy/checkouts.json");
        let mut list = self.checkout_list();
        list["settings"] = settings;
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, list.to_string()).unwrap();
    }

    /// Has `checkout` say `claims` of its concurrency on this world's host,
    /// as a `run` that named `--concurrency` keeps it: with its entry on the
    /// checkout list, which it puts there where the list holds none of it.
    pub fn say_concurrency(&self, checkout: &Path, claims: u64) {
        let file = self.cache().join("afkconvoy/checkouts.json");
        let mut list = self.checkout_list();
        let entries = list["checkouts"].as_array_mut().unwrap();
        match entries.iter_mut().find(|entry| entry["checkout"] == json!(checkout)) {
            Some(entry) => entry["concurrency"] = json!(claims),
            None => entries.push(json!({"checkout": checkout, "concurrency": claims})),
        }
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, list.to_string()).unwrap();
    }

    /// Puts `checkout` on this world's checkout list, as a `run` of it does,
    /// saying nothing of it: its tracker is its own repo.
    pub fn list_checkout(&self, checkout: &Path) {
        let file = self.cache().join("afkconvoy/checkouts.json");
        let mut list = self.checkout_list();
        let entries = list["checkouts"].as_array_mut().unwrap();
        if !entries.iter().any(|entry| entry["checkout"] == json!(checkout)) {
            entries.push(json!({"checkout": checkout}));
        }
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, list.to_string()).unwrap();
    }

    /// Has the remote of `checkout` be named as the repo `repo` (`owner/repo`)
    /// on GitHub, as an issue of it in another tracker names it: git reads
    /// that name as this world's remote all the same.
    pub fn name_origin(&self, checkout: &Path, repo: &str) {
        let url = format!("https://github.com/{repo}.git");
        git(checkout, &["config", "remote.origin.url", &url]);
        git(checkout, &["config", &format!("url.{}.insteadOf", self.bare.display()), &url]);
    }

    /// Puts the repo `tracker` (`owner/repo`) on the stand-in GitHub, its
    /// issues kept apart from this repo's in a state of their own, which
    /// every call naming it reads and writes, where it is not there yet; and
    /// has `checkout` say it is its tracker, with its entry on this world's
    /// checkout list.
    pub fn say_tracker(&self, checkout: &Path, tracker: &str) {
        self.put_tracker(tracker);
        let file = self.cache().join("afkconvoy/checkouts.json");
        let mut list = self.checkout_list();
        let entries = list["checkouts"].as_array_mut().unwrap();
        match entries.iter_mut().find(|entry| entry["checkout"] == json!(checkout)) {
            Some(entry) => entry["tracker"] = json!(tracker),
            None => entries.push(json!({"checkout": checkout, "tracker": tracker})),
        }
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, list.to_string()).unwrap();
    }

    /// Puts the repo `tracker` (`owner/repo`) on the stand-in GitHub, its
    /// issues kept apart from this repo's in a state of their own, which
    /// every call naming it reads and writes, where it is not there yet.
    pub fn put_tracker(&self, tracker: &str) {
        let state = json!({"issues": [], "comments": {}, "labels": ["ready-for-agent"],
            "calls": [], "envs": []});
        let file = self.tracker_file(tracker);
        if !file.exists() {
            std::fs::write(&file, state.to_string()).unwrap();
        }
        self.with_gh(|gh| gh["trackers"][tracker] = json!(file));
    }

    fn tracker_file(&self, tracker: &str) -> PathBuf {
        self.root.join(format!("tracker-{}.json", tracker.replace('/', "-")))
    }

    /// The one tracker of a world that says one (`say_tracker`).
    fn the_tracker(&self) -> String {
        let trackers = self.gh()["trackers"].as_object().cloned().unwrap_or_default();
        assert_eq!(trackers.len(), 1, "one tracker: {trackers:?}");
        trackers.keys().next().unwrap().clone()
    }

    /// The stand-in GitHub's state of the tracker (`say_tracker`), of a
    /// world that says one.
    pub fn tracker(&self) -> Value {
        self.tracker_of(&self.the_tracker())
    }

    /// The stand-in GitHub's state of the tracker `tracker` (`say_tracker`).
    pub fn tracker_of(&self, tracker: &str) -> Value {
        read(&self.tracker_file(tracker))
    }

    /// Changes the state of the tracker (`say_tracker`) in place, of a world
    /// that says one.
    pub fn with_tracker(&self, change: impl FnOnce(&mut Value)) {
        self.with_tracker_of(&self.the_tracker(), change);
    }

    /// Changes the state of the tracker `tracker` (`say_tracker`) in place.
    pub fn with_tracker_of(&self, tracker: &str, change: impl FnOnce(&mut Value)) {
        let mut state = self.tracker_of(tracker);
        change(&mut state);
        std::fs::write(self.tracker_file(tracker), state.to_string()).unwrap();
    }

    /// Every line of the host runner's log, the stamp of each taken off.
    pub fn runner_log(&self) -> Vec<String> {
        let file = self.cache().join("afkconvoy/host-runner.log");
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let stamped = |line: &str| {
            let bytes = line.as_bytes();
            bytes.len() > 21 && bytes[4] == b'-' && bytes[10] == b'T' && &bytes[19..21] == b"Z "
        };
        let unstamped =
            |line: &str| if stamped(line) { line[21..].to_string() } else { line.into() };
        text.lines().map(unstamped).collect()
    }

    /// The pid of this world's host runner: the one its lock says, while
    /// that lock is held.
    pub fn host_runner(&self) -> Option<u32> {
        if !self.runner_lock_held() {
            return None;
        }
        let text = std::fs::read_to_string(self.cache().join("afkconvoy/host-runner.lock"));
        let holder: Value = serde_json::from_str(&text.ok()?).ok()?;
        holder["pid"].as_u64().map(|pid| pid as u32)
    }

    /// Whether a process holds this world's host runner's lock.
    fn runner_lock_held(&self) -> bool {
        let file = self.cache().join("afkconvoy/host-runner.lock");
        let Ok(lock) = std::fs::File::open(file) else { return false };
        matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock))
    }

    /// Starts a host runner of the afkconvoy `version` on this world's host
    /// (the `fake_host_runner` stand-in), and waits until it holds the host
    /// and listens → its process, which the world kills as it ends.
    pub fn host_runner_of(&self, version: &str) -> Child {
        let built = Path::new(env!("CARGO_BIN_EXE_afkconvoy")).parent().unwrap().join("examples");
        let mut child = self
            .command(built.join("fake_host_runner"), &self.root)
            .arg(version)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let socket = self.cache().join("afkconvoy/host-runner.sock");
        let until = Instant::now() + Duration::from_secs(30);
        while self.host_runner() != Some(child.id()) || !socket.exists() {
            assert!(child.try_wait().unwrap().is_none(), "the host runner of {version} ended");
            assert!(Instant::now() < until, "the host runner of {version} did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        child
    }

    /// Sends this world's host runner `signal` (`TERM`, `KILL`) and waits up
    /// to 30 seconds for it to let the host go.
    pub fn kill_host_runner(&self, signal: &str) {
        let pid = self.host_runner().expect("a host runner holds this world's host");
        let sent = Command::new("kill").arg(format!("-{signal}")).arg(pid.to_string()).status();
        assert!(sent.unwrap().success(), "kill -{signal} {pid}");
        let until = Instant::now() + Duration::from_secs(30);
        while self.runner_lock_held() {
            assert!(
                Instant::now() < until,
                "host runner {pid} still holds the host 30s after {signal}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// TypeSafe's HTTP endpoint: curl's arguments on the first line of
/// `typesafe-request`, and the config it reads on its standard input — the
/// URL, the headers and the payload — after them, as the evidence a test
/// asserts against. No real network call is made.
const CURL: &str = r#"#!/bin/sh
root=${0%/*}/..
{ printf '%s\n' "$*"; cat; } > "$root/typesafe-request"
if [ -f "$root/typesafe-fail" ]; then cat "$root/typesafe-fail" >&2; exit 22; fi
cat "$root/typesafe-answer"
"#;

/// The login shell a worker would get, as far as the fleet asks it anything.
/// A question wrapped between its marks is unwrapped first: the inner script
/// is what is logged and answered, and the marks are printed around that
/// answer. Otherwise:
/// `shell -lic 'alias'`, `shell -lic "type -- '<word>'"`, `shell -lic '<command> --help'` —
/// where `pi` answers as pi does, `codex` as Codex and every other command as Claude Code —
/// a model's probe, which prints nothing and fails unless a `probes-answer` beside
/// `bin/` is there,
/// `shell -lic '… <command> --list-models'`, which fails unless a `shell-models` beside
/// `bin/` holds what it prints, `shell -lic '… <command> debug models'`, which
/// fails unless a `shell-server-models` there does, and whether `gh`, `claude`, `pi` and `codex` are logged in. Every
/// word is a program unless `shell-types`
/// beside `bin/` says otherwise, and each is logged in unless a
/// `logged-out-<tool>` there says otherwise. It needs nothing of PATH. Asked
/// for its environment (`AFKCONVOY_ENV`), it is a fresh login shell: it reads
/// the world's profile, then runs what it was asked.
const SHELL: &str = r#"#!/bin/sh
root=${0%/*}/..
# A question is wrapped between marks. Answer the inner script, and print the
# marks around that answer. The log records the inner script, once.
case "$2" in
  *"AFKCONVOY_ ASKED; ("*)
    inner=$2
    inner=${inner#*"AFKCONVOY_ ASKED; ( "}
    inner=${inner%" ); printf '\n%s%s %s\n' AFKCONVOY_ ANSWERED \"\$?\""}
    captured=$("$0" -lic "$inner" 2>&1)
    status=$?
    printf 'AFKCONVOY_ASKED\n%s\n\nAFKCONVOY_ANSWERED %s\n' "$captured" "$status"
    exit 0
    ;;
esac
echo "$2" >> "$root/shell-asked"
pwd -P >> "$root/shell-asked-in"
case "$2" in
  *"AFKCONVOY_ENV"*)
    . "$HOME/.profile"; eval "$2"; exit 0 ;;
  *"You are the caster of a fleet"*)
    # A caster's prompt: answered with what `caster-answer` holds.
    printf '%s' "$2" > "$root/caster-asked"
    if [ -f "$root/caster-answer" ]; then cat "$root/caster-answer"; exit 0; fi
    exit 1 ;;
*"Research new coding-agent models"*)
    printf '%s\n' "$2" >> "$root/research-asked"
    if [ -f "$root/research-answer" ]; then cat "$root/research-answer"; exit 0; fi
    exit 1 ;;
  *"You are drafting the roster of a team"*)
    # A drafter's prompt: answered with what `drafter-answer` holds.
    printf '%s' "$2" > "$root/drafter-asked"
    if [ -f "$root/drafter-answer" ]; then cat "$root/drafter-answer"; exit 0; fi
    exit 1 ;;
  *"Reply with the single word OK."*)
    # A recovery probe: answered once `probes-answer` is there.
    if [ -f "$root/probes-answer" ]; then
      echo OK; exit 0
    fi
    exit 1 ;;
  *'"subtype":"initialize"'*)
    # Claude Code asked for its models: answered with `shell-claude-models`.
    if [ -f "$root/shell-claude-models" ]; then cat "$root/shell-claude-models"; exit 0; fi
    exit 1 ;;
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
  "kimi provider list --json")
    if [ -e "$root/logged-out-kimi" ]; then echo '{"models":{}}'; exit 0; fi
    echo '{"models":{"kimi-code/k3":{}}}'; exit 0 ;;
  *" && kimi provider list --json")
    if [ -f "$root/shell-kimi-models" ]; then cat "$root/shell-kimi-models"; exit 0; fi
    echo "command not found" >&2; exit 127 ;;
  "grok models")
    if [ -f "$root/login-first-grok" ]; then
      if [ -e "$root/login-first-answered-grok" ]; then
        cat "$root/login-second-grok"
      else
        : > "$root/login-first-answered-grok"
        cat "$root/login-first-grok"
      fi
      exit 0
    fi
    if [ -e "$root/logged-out-grok" ]; then
      echo "You are not authenticated."
      echo "Default model: grok-4.6"
      echo "Available models:"
      echo "  * grok-4.6 (default)"
      echo "  - grok-4.5"
      exit 0
    fi
    echo "You are logged in with grok.com."
    echo "Default model: grok-4.7"
    echo "Available models:"
    echo "  * grok-4.7 (default)"
    echo "  - grok-4.5"
    exit 0 ;;
  *" && grok --always-approve models"|*" && grok models")
    if [ -f "$root/shell-grok-models" ]; then cat "$root/shell-grok-models"; exit 0; fi
    echo "command not found" >&2; exit 127 ;;
  "printf 'AFKCONVOY_PROXY"*)
    if [ -f "$root/shell-proxy" ]; then cat "$root/shell-proxy"; else printf 'AFKCONVOY_PROXY\000\000'; fi
    exit 0 ;;
  alias) cat "$root/shell-aliases" 2>/dev/null; exit 0 ;;
  "codex "*" --help")
    if [ -f "$root/shell-help" ]; then cat "$root/shell-help"; exit 0; fi
    echo "Usage: codex [OPTIONS] [PROMPT]"
    echo "  -c, --config <key=value>"; echo "  -m, --model <MODEL>"
    echo "      --dangerously-bypass-approvals-and-sandbox"
    echo "      --dangerously-bypass-hook-trust"; echo "      --add-dir <DIR>"; exit 0 ;;
  "dsh-tui --help")
    if [ -f "$root/shell-help" ]; then cat "$root/shell-help"; exit 0; fi
    echo 'Leading DSH options: --patch <path>'; exit 0 ;;
  "command -v dsh")
    exec "$0" -lic "type -- 'dsh'" ;;
  "pi --help")
    echo "Usage:"; echo "  pi [options] [--] [@files...] [messages...]"
    echo "  --model <pattern>              Model pattern or ID"
    echo "  --extension, -e <path>         Load an extension file"
    echo "  --skill <path>                 Load a skill file or directory"
    echo "  --no-skills, -ns               Disable skills discovery and loading"
    echo "  --approve, -a                  Trust project-local files for this run"; exit 0 ;;
  "kimi "*" --help")
    if [ -f "$root/shell-help" ]; then cat "$root/shell-help"; exit 0; fi
    echo "Usage: kimi [options]"
    echo "  --add-dir <dir>"; echo "  --skills-dir <dir>"; echo "  --model <model>"; exit 0 ;;
  "grok "*" --help")
    if [ -f "$root/shell-help" ]; then cat "$root/shell-help"; exit 0; fi
    echo "Usage: grok [options]"
    echo "  --add-dir <directories...>"; echo "  --model <model>"; echo "  --always-approve"; exit 0 ;;
  *" --list-models")
    if [ -f "$root/shell-models" ]; then cat "$root/shell-models"; exit 0; fi
    echo "command not found" >&2; exit 127 ;;
  *" debug models"*)
    if [ -f "$root/shell-server-models" ]; then cat "$root/shell-server-models"; exit 0; fi
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

/// The stand-in `pmset`: what it is asked is written to `pmset.log`, and it
/// puts nothing to sleep. No fleet asks it anything (docs/adr/0050).
const PMSET: &str = r#"#!/bin/sh
echo "$*" >> "${0%/*}/../pmset.log"
"#;

/// The stand-in `sysctl`: for `vm.loadavg`, the load in `host-load`, or none; for
/// `kern.memorystatus_level`, the share of memory free in `host-memory`, or 80%.
/// Each file is one word, read by the shell itself: a fleet asks for its
/// host's load and memory at every tick, and a `cat` for each was a process
/// more each time (sunfmin/control#319).
const SYSCTL: &str = r#"#!/bin/sh
here=${0%/*}/..
echo "$*" >> "$here/sysctl.log"
said() { said=$2; [ -f "$here/$1" ] && read -r said < "$here/$1"; : "${said:=$2}"; }
case "$*" in
*vm.loadavg*) said host-load 0.00; echo "{ $said 0.00 0.00 }" ;;
*kern.memorystatus_level*) said host-memory 80; echo "$said" ;;
*kern.memorystatus_vm_pressure_level*) said host-pressure 1; echo "$said" ;;
*hw.memsize*) echo 34359738368 ;;
*machdep.cpu.brand_string*|*hw.model*|*hw.logicalcpu*) ;;
esac
"#;

/// The stand-in `df`, for admission: a disk of 1000 GiB, half of it free —
/// never the disk of whoever runs the tests.
const DF: &str = r#"#!/bin/sh
echo "$*" >> "${0%/*}/../df.log"
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/stand-in 1048576000 524288000 524288000 50% /"
"#;

/// The stand-in `ps`: the real one, but where asked for the uids of the
/// machine's processes it lists this account's alone, and then the rows in
/// `other-accounts`: of another account's, as a test says them.
const PS: &str = r#"#!/bin/sh
case "$*" in
*pid=,ppid=,pcpu=*)
    if test -f "${0%/*}/../admission-processes"; then
        cat "${0%/*}/../admission-processes"
    else
        exec /bin/ps "$@"
    fi
    ;;
*uid=*)
    me=$(id -u)
    /bin/ps "$@" | awk -v me="$me" '$1 == me'
    cat "${0%/*}/../other-accounts" 2>/dev/null
    ;;
*) exec /bin/ps "$@" ;;
esac
"#;

impl World {
    /// Has the stand-in host's 1-minute load be `load`, as `sysctl` says it.
    pub fn busy(&self, load: &str) {
        std::fs::write(self.root.join("host-load"), load).unwrap();
    }

    /// What the named host command stand-in was asked, one call a line.
    pub fn host_calls(&self, command: &str) -> Vec<String> {
        let log =
            std::fs::read_to_string(self.root.join(format!("{command}.log"))).unwrap_or_default();
        log.lines().map(str::to_string).collect()
    }
}

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

/// An identity for the commits a test makes, and no config of the machine's —
/// and no housekeeping after a commit, a fetch or a push: a world is gone long
/// before it would gain from any, and each is one more process to start.
const GIT_ENV: [(&str, &str); 9] = [
    ("GIT_AUTHOR_NAME", "tester"),
    ("GIT_AUTHOR_EMAIL", "tester@example.com"),
    ("GIT_COMMITTER_NAME", "tester"),
    ("GIT_COMMITTER_EMAIL", "tester@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "maintenance.auto"),
    ("GIT_CONFIG_VALUE_0", "false"),
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

/// A `run` started in the background, its sleeps real: its stdout comes line
/// by line.
pub struct Running {
    child: Child,
    lines: Receiver<String>,
}

impl World {
    /// Starts `run` with `args`, its workers started with `agent`, and goes
    /// on while it runs.
    pub fn running(&self, args: &[&str]) -> Running {
        self.running_in(&self.clone, args)
    }

    /// `running`, of the checkout `dir`.
    pub fn running_in(&self, dir: &Path, args: &[&str]) -> Running {
        let args = [&["run", "--foreground"][..], args].concat();
        let mut child = self
            .command(env!("CARGO_BIN_EXE_afkconvoy"), dir)
            .args(self.settled_args(dir, &args))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (send, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Running { child, lines }
    }
}

impl World {
    /// Holds this world's host with a host runner — a fleet of its second
    /// repo (`second_repo`), beta of h2, held in the foreground, which holds
    /// nothing of the world's own repo — and waits until it listens → its
    /// `run`, which ends as it is dropped. The second repo is made the first
    /// time.
    pub fn host_held(&self) -> Running {
        let clone2 = self.root.join("clone2");
        if !clone2.exists() {
            self.second_repo();
        }
        let running = self.running_in(&clone2, &["--instance", "beta", "--host", "h2"]);
        let socket = self.cache().join("afkconvoy/host-runner.sock");
        let until = Instant::now() + Duration::from_secs(30);
        while self.host_runner() != Some(running.child.id()) || !socket.exists() {
            assert!(
                Instant::now() < until,
                "the host runner held in the foreground never listened"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        running
    }
}

impl Running {
    /// The next line `run` prints but for those it prints whatever it does
    /// (`bookkeeping`), if one comes within `within`.
    pub fn line(&self, within: Duration) -> Option<String> {
        let until = Instant::now() + within;
        loop {
            let line = self.every_line(until.saturating_duration_since(Instant::now()))?;
            if !bookkeeping(&line) {
                return Some(line);
            }
        }
    }

    /// The next line `run` prints, whatever it is, if one comes within `within`.
    pub fn every_line(&self, within: Duration) -> Option<String> {
        self.lines.recv_timeout(within).ok()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether `line` is one a `run` prints whatever its ticks do: the line of
/// its start (`start: run as …`), a tick's (`tick N: started (…)`), or the
/// host's independent watch. Watch events are checked by `host_watch`.
pub fn bookkeeping(line: &str) -> bool {
    let tick_started = line.strip_prefix("tick ").is_some_and(|rest| {
        rest.split_once(": started (").is_some_and(|(count, _)| count.parse::<u64>().is_ok())
    });
    tick_started || line.starts_with("start: run as ") || line.starts_with("watch: ")
}

/// The source worktree is still a worktree, and every stopped candidate's
/// directory is gone. A live candidate is not checked here.
pub fn stopped_candidates_gone(world: &World, source: &Path) {
    assert!(source.is_dir(), "the source worktree stays: {}", source.display());
    let listed = git(&world.clone, &["worktree", "list", "--porcelain"]);
    assert!(worktree_listed(&listed, source), "the source worktree stays listed:\n{listed}");
    let text = world.remote(&["show", "-s", "--format=%B", "refs/convoy/candidates/main"]);
    let chain: Value = serde_json::from_str(&text).unwrap();
    for candidate in chain["candidates"].as_array().unwrap() {
        let phase = candidate["phase"].as_str().unwrap();
        if !matches!(phase, "landed" | "red" | "cancelled") {
            continue;
        }
        let path = PathBuf::from(candidate["worktree"].as_str().unwrap());
        assert!(!path.exists(), "{phase} candidate worktree {} is gone", path.display());
        assert!(
            !worktree_listed(&listed, &path),
            "{phase} candidate {} is still listed:\n{listed}",
            path.display()
        );
    }
}

/// Whether `git worktree list --porcelain` names `path`. A symlink in the
/// path (`/var` and `/private/var`) still counts.
pub fn worktree_listed(listed: &str, path: &Path) -> bool {
    let want = path.to_string_lossy();
    listed.lines().any(|line| {
        let Some(got) = line.strip_prefix("worktree ") else { return false };
        if got == want {
            return true;
        }
        match (std::fs::canonicalize(got), std::fs::canonicalize(path)) {
            (Ok(got), Ok(real)) => got == real,
            _ => false,
        }
    })
}

/// Where the git that Apple's `/usr/bin/git` starts is, when that is the git
/// on PATH: put after the stand-ins on a world's PATH, it is the git the
/// world runs, and the harness's own. `/usr/bin/git` is a shim that asks
/// `xcrun` for the developer directory each time it runs, and then starts
/// the same git: about 16 ms a call where the git alone takes 7 (#234), and
/// a test of the CLI makes about fifty a tick. A symlink to that git will
/// not do: it finds its own `git-core` and templates beside the path it was
/// started as. `None` wherever the git on PATH is another.
static REAL_GIT_DIR: std::sync::LazyLock<Option<PathBuf>> = std::sync::LazyLock::new(|| {
    let path = std::env::var_os("PATH")?;
    let first = std::env::split_paths(&path).map(|dir| dir.join("git")).find(|git| git.is_file());
    if first? != Path::new("/usr/bin/git") {
        return None;
    }
    let found = Command::new("xcrun").args(["--find", "git"]).output().ok()?;
    let real = PathBuf::from(String::from_utf8(found.stdout).ok()?.trim());
    (found.status.success() && real.is_file()).then(|| real.parent().unwrap().to_path_buf())
});

/// The git a world runs, and so the harness's own: see `REAL_GIT_DIR`.
pub fn git_program() -> PathBuf {
    match &*REAL_GIT_DIR {
        Some(dir) => dir.join("git"),
        None => {
            let path = std::env::var_os("PATH").unwrap();
            let git =
                std::env::split_paths(&path).map(|dir| dir.join("git")).find(|git| git.is_file());
            git.expect("git is on PATH")
        }
    }
}

/// Runs git in `cwd` and returns its trimmed stdout; a failure fails the test.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out =
        Command::new(git_program()).current_dir(cwd).args(args).envs(GIT_ENV).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The commands `brief` gives a worker that refuse a worker's worktree as
/// their checkout, as each one's help says: of every word after `afkconvoy `
/// in it that is a command, unless it explicitly names a target checkout.
/// The remaining commands run in the worker's worktree, so none should refuse it.
pub fn refusing_a_workers_worktree(brief: &str) -> Vec<String> {
    let word = |after: &str| {
        let end = after.find(|c: char| !(c.is_ascii_lowercase() || c == '-'));
        after[..end.unwrap_or(after.len())].to_string()
    };
    let named = brief.split("afkconvoy ").skip(1).filter_map(|after| {
        let command = after.split(['`', '\n']).next().unwrap_or_default();
        let target = command.contains(" -C <target-checkout>")
            || command.starts_with("flows <target-checkout>");
        (!target).then(|| word(after))
    });
    let mut named: Vec<String> = named.filter(|name| !name.is_empty()).collect();
    named.sort();
    named.dedup();
    named.retain(|name| {
        let help = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args([name, "--help"]).output();
        let help = help.unwrap();
        // A word that is no command answers no help.
        let text = String::from_utf8_lossy(&help.stdout);
        let said = text.split_once("## Which checkout it is about").map(|(_, said)| said);
        let said = said.map(|said| said.split("\n## ").next().unwrap_or_default());
        help.status.success()
            && said.is_some_and(|said| said.contains("A worker's worktree is refused"))
    });
    named
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

/// What the fleet hands orca to start a worker in the worktree `dir` whose
/// own line is `line`: that line under `nice`, with the worktree's `TMPDIR`.
pub fn niced(world: &World, dir: &Path, line: &str) -> String {
    afkconvoy_core::line::niced(&world.tmp(dir).display().to_string(), line)
}

/// The worker's own line in `command`, what orca was handed to start it:
/// what follows starting it under `nice`.
pub fn unniced(command: &str) -> &str {
    let at = command.find(" >/dev/null && ").unwrap_or_else(|| panic!("not niced: {command}"));
    &command[at + " >/dev/null && ".len()..]
}

//! These exercise the real command and real Seatbelt, with platform faults
//! injected only where the command calls the macOS tools.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Run {
    dir: TempDir,
    node: PathBuf,
}

impl Run {
    fn new() -> Self {
        let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("node"))
            .find(|path| path.is_file())
            .expect("native no-node tests need Node on PATH")
            .canonicalize()
            .unwrap();
        Self { dir: tempfile::tempdir().unwrap(), node }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_afkconvoy"));
        command
            .current_dir(self.dir.path())
            .args(["no-node", "--canary-node"])
            .arg(&self.node)
            .env("TMPDIR", self.dir.path());
        command
    }

    fn shell(&self, script: &str) -> Output {
        self.command()
            .args(["--", "/bin/sh", "-c", script, "gate"])
            .arg(&self.node)
            .output()
            .unwrap()
    }

    fn fault(&self, name: &str) -> Command {
        let bin = self.dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let fake = Path::new(env!("CARGO_BIN_EXE_afkconvoy"))
            .parent()
            .unwrap()
            .join("examples/fake_no_node");
        symlink(fake, bin.join(if name == "sandbox" { "sandbox-exec" } else { "log" })).unwrap();
        let state = self.dir.path().join("state.json");
        fs::write(&state, json!({"mode": name, "calls": []}).to_string()).unwrap();
        let mut command = self.command();
        command
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(bin)
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            )
            .env("NO_NODE_FAKE_STATE", state);
        command
    }
}

fn answer(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {out:?}"))
}

fn green(out: Output) {
    let said = answer(&out);
    assert!(out.status.success(), "{said}: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(said["status"], "green", "{said}");
    assert_eq!(said["invocations"], json!([]));
}

fn hit(out: Output, node: &Path) {
    let said = answer(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert_eq!(said["status"], "red", "{said}: {}", String::from_utf8_lossy(&out.stderr));
    let hits = said["invocations"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{said}");
    assert_eq!(hits[0]["path"], node.to_string_lossy().as_ref());
    assert!(!hits[0]["process"].as_str().unwrap().is_empty());
    assert!(hits[0]["pid"].as_u64().unwrap() > 0);
    assert_eq!(said["exit_code"], 0, "the fixture ignored the denied exec");
}

#[test]
fn a_native_command_without_node_is_green_and_streams_output_to_stderr() {
    let run = Run::new();
    let out = run.shell("echo gate-stdout; echo gate-stderr >&2; /usr/bin/true");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("gate-stdout\ngate-stderr\n"), "{stderr}");
    green(out);
}

#[test]
fn ps_observes_a_live_descendants_command_and_environment() {
    let script = r#"
NO_NODE_PS_MARKER=visible /bin/sleep 30 &
pid=$!
trap 'kill "$pid"; wait "$pid" 2>/dev/null' EXIT
/bin/ps eww -p "$pid" -o command=
"#;
    let direct = Command::new("/bin/sh").args(["-c", script]).output().unwrap();
    assert!(direct.status.success(), "{direct:?}");
    let monitored = Run::new().shell(script);
    let observed = String::from_utf8_lossy(&monitored.stderr);
    for expected in ["/bin/sleep 30", "NO_NODE_PS_MARKER=visible"] {
        assert!(String::from_utf8_lossy(&direct.stdout).contains(expected), "{direct:?}");
        assert!(observed.contains(expected), "{monitored:?}");
    }
    green(monitored);
}

#[test]
fn a_command_failure_stays_red_without_node() {
    let out = Run::new().shell("exit 37");
    let said = answer(&out);
    assert!(!out.status.success());
    assert_eq!(said["status"], "red", "{said}");
    assert_eq!(said["exit_code"], 37);
    assert_eq!(said["invocations"], json!([]));
}

#[test]
fn node_by_path_is_reported_despite_an_ignored_failure() {
    let run = Run::new();
    hit(run.shell("node --version || true"), &run.node);
}

#[test]
fn node_by_absolute_path_is_reported_despite_an_ignored_failure() {
    let run = Run::new();
    hit(run.shell("\"$1\" --version || true"), &run.node);
}

#[test]
fn node_by_symlink_reports_its_real_executable() {
    let run = Run::new();
    symlink(&run.node, run.dir.path().join("alias")).unwrap();
    hit(run.shell("./alias --version || true"), &run.node);
}

#[test]
fn an_env_node_shebang_is_reported() {
    let run = Run::new();
    fs::write(run.dir.path().join("script"), "#!/usr/bin/env node\nconsole.log('forbidden')\n")
        .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(run.dir.path().join("script"), fs::Permissions::from_mode(0o755)).unwrap();
    hit(run.shell("./script || true"), &run.node);
}

#[test]
fn a_non_homebrew_node_is_reported_without_an_installation_list() {
    let run = Run::new();
    let dir = run.dir.path().join("unlisted-runtime/bin");
    fs::create_dir_all(&dir).unwrap();
    let node = dir.join("node");
    fs::copy(&run.node, &node).unwrap();
    hit(run.shell("./unlisted-runtime/bin/node --version || true"), &node.canonicalize().unwrap());
}

#[test]
fn a_grandchilds_ignored_denial_is_reported() {
    let run = Run::new();
    hit(run.shell("/bin/sh -c '\"$1\" --version || true' child \"$1\""), &run.node);
}

#[test]
fn a_background_descendant_is_waited_for_before_the_second_canary() {
    let run = Run::new();
    let began = Instant::now();
    hit(run.shell("(/bin/sleep 0.3; \"$1\" --version || true) & exit 0"), &run.node);
    assert!(began.elapsed() >= Duration::from_millis(300));
}

#[test]
fn an_outside_node_running_concurrently_causes_no_hit() {
    let run = Run::new();
    let ready = run.dir.path().join("ready");
    let outside = run.dir.path().join("outside");
    let monitored = run
        .command()
        .args([
            "--",
            "/bin/sh",
            "-c",
            "echo ready > ready; while [ ! -f outside ]; do /bin/sleep 0.01; done",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let began = Instant::now();
    while !ready.exists() {
        assert!(began.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = Command::new(&run.node)
        .args(["-e", "require('fs').writeFileSync(process.argv[1], 'ready')"])
        .arg(&outside)
        .status()
        .unwrap();
    assert!(status.success());
    green(monitored.wait_with_output().unwrap());
}

#[test]
fn node_reports_are_preserved_when_the_command_itself_fails() {
    let run = Run::new();
    let out = run.shell("\"$1\" --version || true; \"$1\" --version || true; exit 37");
    let said = answer(&out);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(said["status"], "red", "{said}");
    assert_eq!(said["exit_code"], 37);
    let hits = said["invocations"].as_array().unwrap();
    assert_eq!(hits.len(), 2, "{said}");
    for hit in hits {
        assert_eq!(hit["path"], run.node.to_string_lossy().as_ref());
    }
}

#[test]
fn bun_run_bun_and_its_node_compatibility_link_are_allowed() {
    let run = Run::new();
    fs::write(run.dir.path().join("app.js"), r#"
if (!process.versions.bun) process.exit(7);
const result = require('child_process').spawnSync('node', ['-e', 'if (!process.versions.bun) process.exit(8)']);
process.stderr.write(result.stderr);
process.exit(result.status ?? 9);
"#).unwrap();
    green(run.command().args(["--", "bun", "run", "--bun", "app.js"]).output().unwrap());
}

#[test]
fn a_nested_sandbox_fails_closed_before_running_the_command() {
    let run = Run::new();
    let out = Command::new("/usr/bin/sandbox-exec")
        .args([
            "-p",
            "(version 1)(allow default)",
            env!("CARGO_BIN_EXE_afkconvoy"),
            "no-node",
            "--canary-node",
        ])
        .arg(&run.node)
        .args(["--", "/usr/bin/true"])
        .env("TMPDIR", run.dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(answer(&out)["error"].as_str().unwrap().contains("sandbox"), "{out:?}");
}

fn fault(name: &str, message: &str) {
    let run = Run::new();
    let out = run.fault(name).args(["--", "/usr/bin/true"]).output().unwrap();
    assert!(!out.status.success(), "{out:?}");
    assert!(answer(&out)["error"].as_str().unwrap().contains(message), "{out:?}");
    let state: Value =
        serde_json::from_slice(&fs::read(run.dir.path().join("state.json")).unwrap()).unwrap();
    assert!(!state["calls"].as_array().unwrap().is_empty());
}

#[test]
fn a_log_reader_returning_nothing_fails_closed() {
    fault("empty", "missing canary");
}
#[test]
fn a_missing_before_canary_fails_closed() {
    fault("missing-before", "before=false");
}
#[test]
fn a_missing_after_canary_fails_closed() {
    fault("missing-after", "after=false");
}
#[test]
fn a_log_reader_error_fails_closed() {
    fault("error", "injected unified log read failure");
}
#[test]
fn malformed_log_output_fails_closed() {
    fault("malformed", "invalid log show JSON");
}
#[test]
fn a_sandbox_startup_failure_fails_closed() {
    fault("sandbox", "sandbox did not complete");
}

#[test]
fn a_timed_out_command_never_returns_green() {
    let run = Run::new();
    let out = run.command().args(["--seconds", "1", "--", "/bin/sleep", "30"]).output().unwrap();
    assert!(!out.status.success());
    assert!(answer(&out)["error"].as_str().unwrap().contains("timed out"), "{out:?}");
}

#[test]
fn an_interrupted_monitor_never_returns_green() {
    let run = Run::new();
    let ready = run.dir.path().join("ready");
    let child = run
        .command()
        .args(["--", "/bin/sh", "-c", "echo ready > ready; /bin/sleep 30"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let began = Instant::now();
    while !ready.exists() {
        assert!(began.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: the owned child has not yet been waited for, so its PID cannot be reused.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(answer(&out)["error"].as_str().unwrap().contains("interrupted"), "{out:?}");
}

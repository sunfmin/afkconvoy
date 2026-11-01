//! One inherited Seatbelt profile and its own nonce-tagged kernel reports.
//! Canary PIDs are recorded by the shells that attempt exec, never inferred
//! from names or from a globally observed parent/child relationship.
use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use serde_json::Value;

pub(crate) fn run(command: &[OsString], node: Option<&Path>, seconds: u64) -> Result<Value> {
    #[cfg(target_os = "macos")]
    return darwin::run(command, node, seconds);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (command, node, seconds);
        anyhow::bail!("no-node requires native macOS Seatbelt and unified logging; run it on macOS")
    }
}

#[cfg(target_os = "macos")]
mod darwin {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use anyhow::{Context, bail, ensure};
    use serde::Deserialize;
    use serde_json::json;

    // The wrapper closes its own scope descriptor only after starting the
    // command. The parent releases its read below only once every inheriting
    // descendant exited, so the second canary really follows the whole tree.
    const WRAPPER: &str = r#"
node=$1; records=$2; scope_fd=$3; shift 3
/bin/sh -c 'echo $$ > "$1"; exec "$2" --version' canary "$records/before" "$node" >/dev/null 2>&1 &
wait $!
"$@" </dev/null &
command_pid=$!
eval "exec ${scope_fd}>&-"
wait "$command_pid"
code=$?
printf '%s\n' "$code" > "$records/exit"
IFS= read -r proceed || exit 125
/bin/sh -c 'echo $$ > "$1"; exec "$2" --version' canary "$records/after" "$node" >/dev/null 2>&1 &
wait $!
exit "$code"
"#;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // An error or a signal must not leave the direct command running. The
    // sandbox itself is inherited even by children that leave this group.
    struct Running {
        child: Child,
        reaped: bool,
    }
    impl Drop for Running {
        fn drop(&mut self) {
            if self.reaped {
                return;
            }
            // SAFETY: negative pid addresses only this child's new group.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }

    struct Signals(Vec<signal_hook::SigId>);
    impl Drop for Signals {
        fn drop(&mut self) {
            for id in self.0.drain(..) {
                signal_hook::low_level::unregister(id);
            }
        }
    }

    #[derive(Deserialize)]
    struct Event {
        #[serde(rename = "eventMessage")]
        message: String,
        #[serde(rename = "processImagePath")]
        source: String,
    }

    pub(super) fn run(command: &[OsString], node: Option<&Path>, seconds: u64) -> Result<Value> {
        let node = canary(node)?;
        let nonce = format!("afkconvoy-no-node-{}", uuid::Uuid::new_v4());
        let dir = std::env::temp_dir().join(&nonce);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .context("create no-node temporary directory")?;
        let scratch = Scratch(dir);
        let dir = &scratch.0;
        let started = crate::process::output(Command::new("date").arg("+%Y-%m-%d %H:%M:%S"))
            .context("cannot start date; put /bin on PATH and rerun")?;
        ensure!(started.status.success(), "date failed; put /bin on PATH and rerun");
        let started = String::from_utf8(started.stdout)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let mut signals = Signals(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM] {
            signals.0.push(signal_hook::flag::register(signal, stopped.clone())?);
        }
        let profile = format!(
            "(version 1)(allow default)(deny process-exec (with message \"{nonce}\") (regex #\"/node$\"))"
        );
        let mut sandbox = Command::new("sandbox-exec");
        sandbox.args(["-p", &profile, "/bin/sh", "-c", WRAPPER, "no-node"]);
        // Scope creation installs the inheritance hook before spawn. Only the
        // sandbox gets this descriptor, not date or the subsequent log reads.
        let scope_path = dir.join("scope");
        let scope = crate::process_scope::create(&scope_path, &mut sandbox)?;
        sandbox
            .arg(&node)
            .arg(dir)
            .arg(scope.as_raw_fd().to_string())
            .args(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(std::io::stderr()))
            .stderr(Stdio::inherit())
            .process_group(0);
        let mut child = Running {
            child: crate::process::spawn(&mut sandbox).context(
                "cannot start sandbox-exec; put /usr/bin on PATH and run outside any enclosing sandbox",
            )?,
            reaped: false,
        };
        // Command retains the pre_exec closure, but that closure owns no File.
        // Closing the parent's copy makes free() an observation of children.
        drop(scope);
        let began = Instant::now();
        let mut released = false;
        let status = loop {
            ensure!(
                !stopped.load(Ordering::Relaxed),
                "no-node interrupted; no green result; rerun"
            );
            ensure!(
                began.elapsed() < Duration::from_secs(seconds),
                "no-node command tree timed out after {seconds}s; fix the command or raise --seconds and rerun"
            );
            if let Some(status) = child.child.try_wait()? {
                child.reaped = true;
                break status;
            }
            if !released && dir.join("exit").exists() && crate::process_scope::free(&scope_path)? {
                child
                    .child
                    .stdin
                    .take()
                    .context("sandbox control pipe missing")?
                    .write_all(b"after\n")?;
                released = true;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        ensure!(
            released,
            "sandbox did not complete its command; exit {status}; run outside any enclosing sandbox and check sandbox-exec"
        );
        let before = pid(&dir.join("before"))?;
        let after = pid(&dir.join("after"))?;
        ensure!(before != after, "canary PIDs were reused; rerun no-node");
        let code: i32 = fs::read_to_string(dir.join("exit"))?.trim().parse()?;
        let wrapper_code = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
        ensure!(wrapper_code == code, "sandbox wrapper was interrupted; no complete result; rerun");
        let invocations = reports(started.trim(), &nonce, before, after, &node, &stopped)?;
        ensure!(!stopped.load(Ordering::Relaxed), "no-node interrupted; no green result; rerun");
        Ok(json!({
            "status": if code == 0 && invocations.is_empty() { "green" } else { "red" },
            "exit_code": code,
            "nonce": nonce,
            "canaries": {"before": before, "after": after},
            "invocations": invocations,
        }))
    }

    fn canary(given: Option<&Path>) -> Result<PathBuf> {
        let candidate = match given {
            Some(path) => path.to_path_buf(),
            None => std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join("node"))
                .find(|path| path.is_file())
                .context(
                    "no canary Node on PATH; rerun with --canary-node /absolute/path/to/node",
                )?,
        };
        let real = candidate.canonicalize().with_context(|| {
            format!(
                "cannot resolve canary {}; rerun with --canary-node /absolute/path/to/node",
                candidate.display()
            )
        })?;
        ensure!(
            real.file_name().is_some_and(|name| name == "node") && real.is_file(),
            "canary must resolve to a binary named node, got {}; use --canary-node /absolute/path/to/node (a Bun compatibility link is not Node)",
            real.display()
        );
        Ok(real)
    }

    fn pid(path: &Path) -> Result<u32> {
        let pid = fs::read_to_string(path)
            .with_context(|| format!("missing {} canary; sandbox did not apply or was interrupted; rerun outside any enclosing sandbox", path.file_name().unwrap().to_string_lossy()))?
            .trim().parse()?;
        ensure!(pid > 0, "invalid canary PID; rerun no-node");
        Ok(pid)
    }

    fn reports(
        started: &str,
        nonce: &str,
        before: u32,
        after: u32,
        node: &Path,
        stopped: &AtomicBool,
    ) -> Result<Vec<Value>> {
        let predicate = format!("process == \"kernel\" AND eventMessage CONTAINS \"{nonce}\"");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            ensure!(
                !stopped.load(Ordering::Relaxed),
                "no-node interrupted while reading reports; rerun"
            );
            let out = crate::process::output_within(
                Command::new("log").args([
                    "show",
                    "--start",
                    started,
                    "--style",
                    "json",
                    "--predicate",
                    &predicate,
                ]),
                Duration::from_secs(5),
            )
            .context("cannot start log show; put /usr/bin on PATH and rerun")?
            .context("log show timed out; check unified logging and rerun")?;
            ensure!(
                out.status.success(),
                "log show failed: {}; check access to unified logging and rerun",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            let events: Vec<Event> = serde_json::from_slice(&out.stdout)
                .context("invalid log show JSON; use macOS /usr/bin/log and rerun")?;
            let mut seen = [false; 2];
            let mut hits = Vec::new();
            for event in events {
                // Never mistake log's own invocation record for a denial.
                ensure!(
                    event.source == "/kernel",
                    "unexpected source in sandbox reports; use macOS /usr/bin/log and rerun"
                );
                let hit = denial(&event.message, nonce)?;
                let canary = [before, after].iter().position(|pid| hit["pid"] == *pid);
                if let Some(index) = canary {
                    ensure!(
                        !seen[index] && hit["path"] == node.to_string_lossy().as_ref(),
                        "unexpected canary report; cannot establish a complete result; rerun"
                    );
                    seen[index] = true;
                } else {
                    hits.push(hit);
                }
            }
            if seen == [true, true] {
                return Ok(hits);
            }
            if Instant::now() >= deadline {
                bail!(
                    "missing canary reports (before={}, after={}); check unified sandbox logging and rerun; no green result",
                    seen[0],
                    seen[1]
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn denial(message: &str, nonce: &str) -> Result<Value> {
        let (line, tag) =
            message.split_once('\n').context("sandbox report has no nonce line; rerun")?;
        ensure!(
            tag == nonce,
            "unexpected sandbox nonce; cannot establish a complete result; rerun"
        );
        let (caller, path) = line
            .split_once(" deny(1) process-exec* ")
            .context("unrecognized sandbox denial; cannot establish a complete result; rerun")?;
        let caller =
            caller.strip_prefix("Sandbox: ").context("unrecognized sandbox caller; rerun")?;
        let (process, pid) = caller.rsplit_once('(').context("sandbox caller has no PID; rerun")?;
        let pid: u32 = pid.strip_suffix(')').context("invalid sandbox PID; rerun")?.parse()?;
        ensure!(
            path.starts_with('/') && path.ends_with("/node"),
            "unexpected executable in sandbox report; rerun"
        );
        Ok(json!({"process": process, "pid": pid, "path": path}))
    }
}

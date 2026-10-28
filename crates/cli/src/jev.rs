//! Jev, TypeSafe's System One model, asked what a stopped worker's screen
//! shows (`afkconvoy_core::worker_health` holds the questions and the table).
//! The key command is a host setting, run afresh for each check. The key it
//! prints lives only in this process and in what curl reads on its standard
//! input: it is in no file, argument, answer or log line.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::worker_health::{self, Decision, Evidence};
use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::host_settings;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// How long the key command, and then the call to TypeSafe, may each take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What one health check came to.
pub enum Outcome {
    /// The host settings name no Jev key command: nothing was asked.
    NoKey,
    /// Jev answered: `decision` is the one decision its answers settle on,
    /// or why they settle on none.
    Judged { request: Value, response: Value, decision: Result<Decision, String> },
    /// The key command, the call or its answer failed: an error, never an
    /// answer. `request` is null where none was built.
    Failed { request: Value, response: Value, error: String },
}

/// Asks Jev about `screen`, where the host settings name a key command.
pub fn judge(screen: &[String]) -> Outcome {
    let failed = |request, response, error| Outcome::Failed { request, response, error };
    let command = match host_settings::of_host() {
        Ok(settings) => match settings.jev_key_command {
            Some(command) => command,
            None => return Outcome::NoKey,
        },
        Err(error) => return failed(Value::Null, Value::Null, format!("{error:#}")),
    };
    let request = match worker_health::request(screen) {
        Ok(request) => request,
        Err(error) => return failed(Value::Null, Value::Null, error),
    };
    let key = match key(&command) {
        Ok(key) => key,
        Err(error) => return failed(request, Value::Null, format!("{error:#}")),
    };
    let response = match post(&key, &request) {
        Ok(response) => response,
        Err(error) => return failed(request, Value::Null, format!("{error:#}")),
    };
    match worker_health::answers(&request, &response) {
        Ok(answers) => {
            let decision = worker_health::decide(Evidence::Judged(&answers))
                .and_then(|decision| decision.ok_or_else(|| "Jev decided nothing".to_string()));
            Outcome::Judged { decision, request, response }
        }
        Err(error) => failed(request, response, error),
    }
}

/// What `command` prints, trimmed: the key. One that fails, prints nothing,
/// or does not end within `TIMEOUT` is an error, which never holds what it
/// printed.
fn key(command: &str) -> Result<String> {
    let mut child = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("the Jev key command could not be started")?;
    let mut printed = child.stdout.take().context("the Jev key command has no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = printed.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if started.elapsed() >= TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("the Jev key command did not end within {}s", TIMEOUT.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    if !status.success() {
        bail!("the Jev key command failed: {status}");
    }
    let printed = reader.join().unwrap_or_default();
    let key = String::from_utf8(printed).unwrap_or_default().trim().to_string();
    if key.is_empty() {
        bail!("the Jev key command printed no key");
    }
    if key.chars().any(char::is_whitespace) {
        bail!("the Jev key command printed more than one word");
    }
    Ok(key)
}

/// TypeSafe's answer to `request`. curl reads the key and the payload from
/// its standard input, as its config, so neither is among its arguments.
fn post(key: &str, request: &Value) -> Result<Value> {
    let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    let config = format!(
        "url = {}\nheader = \"Content-Type: application/json\"\nheader = {}\ndata-binary = {}\n",
        quoted(ENDPOINT),
        quoted(&format!("Authorization: Bearer {key}")),
        quoted(&serde_json::to_string(request)?),
    );
    let mut child = Command::new("curl")
        .args(["--silent", "--show-error", "--fail-with-body", "--max-time"])
        .arg(TIMEOUT.as_secs().to_string())
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("curl could not be started for the TypeSafe request")?;
    child.stdin.take().context("curl has no stdin")?.write_all(config.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let said = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
        bail!("the TypeSafe request failed: {}", said.trim().replace(key, "[the key]"));
    }
    serde_json::from_slice(&output.stdout).context("TypeSafe answered with no JSON")
}

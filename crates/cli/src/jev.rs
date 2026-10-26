//! TypeSafe's Jev for worker health. The key command is read and run afresh
//! for each check. Its stdout lives only in this process and in the request's
//! Authorization header; payloads, answers and errors never contain the key.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use afkconvoy_core::worker_health::{self, Answers, Decision};
use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::host_settings;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const TIMEOUT: Duration = Duration::from_secs(30);

/// One completed check, with everything the tick log may replay and the pure
/// table's decision.
pub struct Judgment {
    pub request: Value,
    pub answers: Value,
    pub decision: Result<Decision, String>,
}

/// What one check came to. A failed call keeps its attempted request and any
/// response so the tick log can replay it; no key command means no request.
pub enum Outcome {
    NoKey,
    Judged(Judgment),
    Failed { request: Value, answers: Value, error: String },
}

pub fn judge(screen: &[String]) -> Result<Outcome> {
    let Some(command) = host_settings::of_host()?.jev_key_command else {
        return Ok(Outcome::NoKey);
    };
    let key = key(&command)?;
    let request = worker_health::request(screen);
    let response = match post(&key, &request) {
        Ok(response) => response,
        Err(error) => {
            return Ok(Outcome::Failed {
                request,
                answers: Value::Null,
                error: format!("{error:#}"),
            });
        }
    };
    let decision = response
        .get("answers")
        .context("TypeSafe returned no answers")
        .and_then(|answers| answers_of(answers, &request))
        .map(|answers| worker_health::classify(&answers));
    match decision {
        Ok(decision) => Ok(Outcome::Judged(Judgment { request, answers: response, decision })),
        Err(error) => Ok(Outcome::Failed {
            request,
            answers: response,
            error: format!("{error:#}"),
        }),
    }
}

fn key(command: &str) -> Result<String> {
    let output = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("the Jev key command could not be started")?;
    if !output.status.success() {
        bail!("the Jev key command failed with {}", output.status);
    }
    let key =
        String::from_utf8(output.stdout).context("the Jev key command printed no UTF-8 key")?;
    let key = key.trim().to_string();
    if key.is_empty() {
        bail!("the Jev key command printed no key");
    }
    Ok(key)
}

fn post(key: &str, request: &Value) -> Result<Value> {
    let payload = serde_json::to_vec(request)?;
    let mut child = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail-with-body",
            "--max-time",
            &TIMEOUT.as_secs().to_string(),
            "--request",
            "POST",
            "--url",
            ENDPOINT,
            "--header",
            "Content-Type: application/json",
            "--header",
            &format!("Authorization: Bearer {key}"),
            "--data-binary",
            "@-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("curl could not be started for the TypeSafe request")?;
    child.stdin.take().context("curl has no stdin")?.write_all(&payload)?;
    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let detail = format!("{} {}", stderr.trim(), stdout.trim());
        bail!("the TypeSafe request failed: {}", detail.trim().replace(key, "[redacted]"));
    }
    serde_json::from_slice(&output.stdout).context("TypeSafe returned invalid JSON")
}

fn answers_of(answers: &Value, request: &Value) -> Result<Answers> {
    let noul = |id: &str| -> Result<u64> {
        let probability = answers
            .get(id)
            .and_then(|answer| answer.get("noul"))
            .and_then(Value::as_f64)
            .with_context(|| format!("TypeSafe returned no Noul for {id}"))?;
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            bail!("TypeSafe returned an out-of-range Noul for {id}");
        }
        Ok((probability * 1000.0).round() as u64)
    };
    let evidence = answers
        .get(worker_health::EVIDENCE)
        .and_then(|answer| answer.get("choice"))
        .and_then(Value::as_str)
        .context("TypeSafe returned no evidence Choice")?
        .to_string();
    if request["questions"][worker_health::EVIDENCE]["criteria"].get(&evidence).is_none() {
        bail!("TypeSafe selected evidence that is not a screen line");
    }
    Ok(Answers {
        usage_limit: noul(worker_health::USAGE_LIMIT)?,
        busy: noul(worker_health::BUSY)?,
        finished: noul(worker_health::FINISHED)?,
        waiting_for_person: noul(worker_health::WAITING_FOR_PERSON)?,
        trust_prompt: noul(worker_health::TRUST_PROMPT)?,
        login_refused: noul(worker_health::LOGIN_REFUSED)?,
        evidence,
    })
}

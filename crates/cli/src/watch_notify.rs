//! Human decisions leave the host only through its chosen command. The
//! durable outbox lives beside watch reports; a separate sender never holds
//! the watch's scheduler lock while running a command.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::watch_report::{Human, Report};

pub const SECONDS: u64 = 30;
pub const FAILURES: u64 = 3;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Outbox {
    reports: BTreeSet<String>,
    look: u64,
    entries: BTreeMap<String, Entry>,
    last: Option<Sending>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    watch: String,
    human: Human,
    sent: bool,
    failures: u64,
    attempted_look: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct Sending {
    watch: String,
    answer: String,
    command: String,
    at: u64,
    ended: Option<u64>,
    outcome: String,
}

fn change<T>(f: impl FnOnce(&mut Outbox) -> T) -> Result<T> {
    let directory = crate::copies::cache()?.join("watch");
    std::fs::create_dir_all(&directory)?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("notify.lock"))?;
    lock.lock()?;
    let mut outbox = read()?;
    let answer = f(&mut outbox);
    let temporary = directory.join(format!("notify-{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&outbox)?)?;
    std::fs::rename(temporary, directory.join("notify.json"))?;
    Ok(answer)
}

fn read() -> Result<Outbox> {
    match std::fs::read(crate::copies::cache()?.join("watch/notify.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Outbox::default()),
        Err(error) => Err(error.into()),
    }
}

/// Replaying one report is harmless, including after its decisions were
/// cleared by a later report. An absent answer ends its sending episode.
pub fn reported(watch: &str, report: &Report) -> Result<()> {
    change(|outbox| {
        if !outbox.reports.insert(watch.into()) {
            return;
        }
        outbox.entries.retain(|answer, _| report.human.iter().any(|h| &h.answer == answer));
        for human in &report.human {
            outbox.entries.entry(human.answer.clone()).or_insert_with(|| Entry {
                id: crate::unique(),
                watch: watch.into(),
                human: human.clone(),
                sent: false,
                failures: 0,
                attempted_look: None,
            });
        }
    })
}

/// A completed look permits one more attempt for each unsent decision.
pub fn looked() -> Result<()> {
    change(|outbox| outbox.look = outbox.look.saturating_add(1))
}

pub fn run(ended: &AtomicBool, panel: Option<&str>, say: impl Fn(String)) {
    while !ended.load(Ordering::Relaxed) {
        if let Err(error) = step(ended, panel, Duration::from_secs(SECONDS)) {
            say(format!("human decision sending failed: {error:#}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn step(ended: &AtomicBool, panel: Option<&str>, bound: Duration) -> Result<()> {
    let settings = crate::host_settings::of_host()?;
    let Some(command) = settings.notify_command else { return Ok(()) };
    let outbox = read()?;
    if !outbox
        .entries
        .values()
        .any(|entry| !entry.sent && entry.attempted_look != Some(outbox.look))
    {
        return Ok(());
    }
    let entry = change(|outbox| {
        let entry = outbox
            .entries
            .values_mut()
            .find(|entry| !entry.sent && entry.attempted_look != Some(outbox.look))?;
        // Claim before acting. A restart retries an interrupted attempt at
        // the next look rather than starting it twice in one look.
        entry.attempted_look = Some(outbox.look);
        outbox.last = Some(Sending {
            watch: entry.watch.clone(),
            answer: entry.human.answer.clone(),
            command: command.clone(),
            at: crate::clock(),
            ended: None,
            outcome: "running".into(),
        });
        Some(entry.clone())
    })?;
    let Some(entry) = entry else { return Ok(()) };
    let payload = json!({
        "host": settings.host_name.unwrap_or_else(crate::host_settings::host_name),
        "watch": entry.watch, "question": entry.human.question, "answer": entry.human.answer,
        "panel": panel,
    });
    let result = send(&command, &payload, ended, bound);
    change(|outbox| {
        if let Some(current) = outbox.entries.get_mut(&entry.human.answer)
            && current.id == entry.id
        {
            current.sent = result.is_ok();
            current.failures = if current.sent { 0 } else { current.failures.saturating_add(1) };
        }
        outbox.last = Some(Sending {
            watch: entry.watch,
            answer: entry.human.answer,
            command,
            at: outbox.last.as_ref().map_or_else(crate::clock, |last| last.at),
            ended: Some(crate::clock()),
            outcome: result.map_or_else(|error| format!("{error:#}"), |()| "sent".into()),
        });
    })
}

fn send(command: &str, payload: &Value, ended: &AtomicBool, bound: Duration) -> Result<()> {
    let mut child = Command::new("sh")
        .args(["-c", command])
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("the notify command could not be started")?;
    // Writing even a large report must not block the deadline. Closing the
    // process group's pipe on timeout also releases this writer.
    let mut stdin = child.stdin.take().context("the notify command has no stdin")?;
    let mut bytes = serde_json::to_vec(payload)?;
    bytes.push(b'\n');
    let (written, writing) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = written.send(stdin.write_all(&bytes));
    });
    let started = Instant::now();
    let mut status = None;
    let mut input = None;
    let result = loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(ended) => status = ended,
                Err(error) => break Err(error.into()),
            }
        }
        if input.is_none() {
            input = writing.try_recv().ok();
        }
        if let Some(status) = status {
            if !status.success() {
                break Err(anyhow::anyhow!("the notify command failed: {status}"));
            }
            if let Some(input) = input {
                break input.map_err(Into::into);
            }
        }
        if ended.load(Ordering::Relaxed) {
            break Err(anyhow::anyhow!("sending interrupted"));
        }
        if started.elapsed() >= bound {
            break Err(anyhow::anyhow!(
                "the notify command timed out after {} seconds",
                bound.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    // SAFETY: only the dedicated group started above is signalled. Clean up
    // descendants even if their shell exited without waiting for them.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    result
}

pub fn json() -> Value {
    match read() {
        Ok(outbox) => {
            json!({"command": crate::host_settings::of_host().ok().and_then(|s| s.notify_command),
            "last": outbox.last, "entries": outbox.entries.values().collect::<Vec<_>>() })
        }
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(settings) = crate::host_settings::of_host() else { return vec![] };
    if settings.notify_command.is_none() {
        return vec![];
    }
    match read() {
        Ok(outbox) => outbox
            .entries
            .values()
            .filter(|entry| entry.failures >= FAILURES)
            .map(|entry| {
                crate::doctor::finding(
                    crate::doctor::Kind::NotifyFailed,
                    json!(["notify", entry.id]),
                    "human decision sending has failed repeatedly".into(),
                    format!(
                        "{} consecutive failures; answer: {}",
                        entry.failures, entry.human.answer
                    ),
                    "afkconvoy run --notify-command '<corrected command>'".into(),
                )
            })
            .collect(),
        Err(error) => vec![crate::doctor::finding(
            crate::doctor::Kind::NotifyFailed,
            json!(["notify-cache"]),
            "human decision sending has failed repeatedly: its cache cannot be read".into(),
            format!("{error:#}"),
            "afkconvoy doctor --all".into(),
        )],
    }
}

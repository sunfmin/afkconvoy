//! The asking session's launch and delivery journal. The title discovers a
//! Terminal; the persisted session and public pane identity prove ownership.

use std::path::PathBuf;

use afkconvoy_core::line::Line;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host_asking::{Queued, Session};
use crate::{copies::cache, orca::Worktree};

pub(crate) const TITLE: &str = "afkconvoy asks";
const FILE: &str = "asking/terminal.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaunchState {
    Creating,
    Ready,
    Sending,
    Running,
    Idle,
    Ended,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Record {
    pub session: Session,
    pub status: LaunchState,
    pub question: Option<Queued>,
    pub terminal: Option<Value>,
    pub problem: Option<String>,
    pub command: String,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub receipt: Option<Value>,
    #[serde(default)]
    pub offered: Option<u64>,
    #[serde(default)]
    pub execution_evidence: Option<Value>,
}

fn file() -> Result<PathBuf> {
    Ok(cache()?.join(FILE))
}

pub(crate) fn read() -> Result<Option<Record>> {
    let path = file()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("cannot read asking association {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The current asking Terminal, also during the creation-reply window. An
/// ended session protects none. No title or directory establishes ownership.
pub(crate) fn protected() -> Result<Option<String>> {
    let Some(record) = read()? else { return Ok(None) };
    if record.status == LaunchState::Ended {
        return Ok(None);
    }
    if !record.session.terminal.is_empty() {
        return Ok(Some(record.session.terminal));
    }
    match std::fs::read_to_string(handshake(&record.session)) {
        Ok(handle) => Ok(Some(handle.trim().to_string()).filter(|handle| !handle.is_empty())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn save(record: &Record) -> Result<()> {
    let path = file()?;
    std::fs::create_dir_all(path.parent().expect("asking association parent"))?;
    let temporary = path.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(record)?)?;
    std::fs::rename(temporary, path)?;
    if let Some(question) = &record.question {
        let directory = cache()?.join("asking/questions").join(&question.question.id);
        std::fs::create_dir_all(&directory)?;
        let delivery = directory.join("delivery.json");
        let temporary = delivery.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&json!({
                "question": question.question.id,
                "session": record.session.id,
                "terminal": record.session.terminal,
                "status": record.status,
                "request": record.request,
                "receipt": record.receipt,
                "offered": record.offered,
                "execution_evidence": record.execution_evidence,
            }))?,
        )?;
        std::fs::rename(temporary, delivery)?;
    }
    Ok(())
}

fn handshake(session: &Session) -> PathBuf {
    session.transcript.with_extension("terminal")
}

fn claim(session: &Session) -> PathBuf {
    session.transcript.with_extension("claim")
}

fn matches(record: &Record, session: &Session) -> bool {
    record.session.id == session.id
}

pub(crate) fn prepare(session: &Session, command: &str, question: &Queued) -> Result<()> {
    ensure!(
        read()?.is_none_or(|record| record.status == LaunchState::Ended),
        "an asking launch is still unresolved; no second agent started"
    );
    let worktree = Worktree::at(session.directory.clone(), String::new());
    let candidates: Vec<_> =
        worktree.watch_terminals()?.into_iter().filter(|t| t["title"] == TITLE).collect();
    ensure!(
        candidates.is_empty(),
        "unverified terminals named {TITLE}: {}; no agent started",
        candidates
            .iter()
            .map(|t| t["handle"].as_str().unwrap_or("unknown"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    save(&Record {
        session: session.clone(),
        status: LaunchState::Creating,
        question: Some(question.clone()),
        terminal: None,
        problem: None,
        command: command.into(),
        request: None,
        receipt: None,
        offered: None,
        execution_evidence: None,
    })
}

pub(crate) fn launch_line(session: &Session, line: &str) -> String {
    let claim = claim(session).display().to_string();
    let receipt = handshake(session).display().to_string();
    let temporary = format!("{receipt}.tmp");
    Line::new()
        .fixed("if mkdir ").word(&claim)
        .fixed("; then if [ -n \"${ORCA_TERMINAL_HANDLE-}\" ]; then printf '%s\\n' \"$ORCA_TERMINAL_HANDLE\" > ")
        .word(&temporary).fixed(" && mv ").word(&temporary).fixed(" ").word(&receipt)
        .fixed(" && { ").launch(line)
        .fixed("; }; else printf '%s\\n' 'asking launch has no ORCA_TERMINAL_HANDLE; recovery required' >&2; fi; else printf '%s\\n' 'asking launch already claimed; no second agent started' >&2; fi")
        .done()
}

pub(crate) fn created(session: &Session, handle: &str) -> Result<()> {
    let mut record = read()?.context("asking launch has no intent")?;
    ensure!(matches(&record, session), "asking launch intent changed");
    let worktree = Worktree::at(session.directory.clone(), String::new());
    let rows = worktree.watch_terminals()?;
    let row = rows
        .into_iter()
        .find(|row| row["handle"] == handle)
        .context("created asking terminal is not listed; recovery required")?;
    record.session.terminal = handle.into();
    record.terminal = Some(row);
    record.status = LaunchState::Ready;
    save(&record)
}

pub(crate) fn locate(session: &Session) -> Result<Option<String>> {
    let worktree = Worktree::at(session.directory.clone(), String::new());
    let rows = worktree.watch_terminals()?;
    let record = read()?.filter(|record| matches(record, session));
    let metadata = record.as_ref().and_then(|r| r.terminal.as_ref());
    let exact = if session.terminal.is_empty() {
        record.as_ref().map_or("", |r| r.session.terminal.as_str())
    } else {
        &session.terminal
    };
    if !exact.is_empty()
        && let Some(row) = rows.iter().find(|row| row["handle"] == exact)
    {
        if let Some(saved) = metadata {
            for key in ["ptyId", "incarnationId"] {
                ensure!(
                    saved[key].is_null() || saved[key] == row[key],
                    "asking terminal {} changed {key}; ownership is unverified",
                    session.terminal
                );
            }
        }
        return Ok(Some(exact.to_string()));
    }
    let acknowledged =
        claim(session).is_dir().then(|| std::fs::read_to_string(handshake(session)).ok()).flatten();
    let candidates: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["title"] == TITLE
                || metadata.is_some_and(|saved| {
                    !saved["ptyId"].is_null() && saved["ptyId"] == row["ptyId"]
                })
                || acknowledged.as_ref().is_some_and(|handle| row["handle"] == handle.trim())
        })
        .collect();
    if candidates.is_empty() {
        if !exact.is_empty() {
            return Ok(None);
        }
        bail!(
            "asking creation has no receipt or confirmed terminal; creation will not be repeated"
        );
    }
    ensure!(
        candidates.len() == 1,
        "multiple asking terminal candidates: {}; recovery required",
        candidates
            .iter()
            .map(|r| r["handle"].as_str().unwrap_or("unknown"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let row = candidates[0];
    let owned = metadata.is_some_and(|saved| {
        !saved["ptyId"].is_null()
            && saved["ptyId"] == row["ptyId"]
            && saved["incarnationId"] == row["incarnationId"]
    }) || acknowledged.as_ref().is_some_and(|handle| row["handle"] == handle.trim());
    ensure!(owned, "terminal {} is named {TITLE} but its ownership is unverified", row["handle"]);
    let handle = row["handle"].as_str().context("asking terminal has no handle")?.to_string();
    if let Some(mut record) = record {
        record.session.terminal = handle.clone();
        record.terminal = Some(row.clone());
        if record.status == LaunchState::Creating {
            record.status = LaunchState::Ready;
        }
        save(&record)?;
    }
    Ok(Some(handle))
}

pub(crate) fn set_status(session: &Session, status: LaunchState) -> Result<()> {
    if let Some(mut record) = read()?.filter(|r| matches(r, session)) {
        record.session = session.clone();
        record.status = status;
        record.problem = None;
        save(&record)?;
    }
    Ok(())
}

pub(crate) fn begin_question(session: &Session, question: &Queued) -> Result<()> {
    let mut record = read()?.context("the resident asking session has no launch journal")?;
    ensure!(matches(&record, session), "the asking session changed; nothing sent");
    ensure!(
        record.status != LaunchState::Creating && record.status != LaunchState::Sending,
        "the asking session has an unresolved launch or delivery; nothing sent"
    );
    record.question = Some(question.clone());
    record.status = LaunchState::Ready;
    record.request = None;
    record.receipt = None;
    record.offered = None;
    record.execution_evidence = None;
    save(&record)
}

pub(crate) fn recovery(why: &str) -> String {
    format!(
        "{why}. No replacement is started and existing work is retained. Run `afkconvoy doctor --all` and `orca terminal list --worktree path:<watch-directory> --json`; inspect candidates with `orca terminal show --terminal <handle> --json`. Rename unrelated same-name terminals with `orca terminal rename --terminal <handle> --title <title> --json`, then let the runner retry. Keep the brief, transcript and launch claim; do not resend creation or send a fresh prompt. The runner may replay only its exact saved Terminal request key."
    )
}

pub(crate) fn json() -> Value {
    match read() {
        Ok(Some(record)) => json!({"title": TITLE, "session": record.session.id,
            "directory": record.session.directory, "terminal": record.session.terminal,
            "identity": record.terminal, "status": record.status, "problem": record.problem,
            "question": record.question, "request": record.request, "receipt": record.receipt}),
        Ok(None) => json!({"title": TITLE, "status": "unassociated"}),
        Err(error) => {
            json!({"title": TITLE, "status": "uncertain", "problem": recovery(&format!("{error:#}"))})
        }
    }
}

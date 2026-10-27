//! The watch's launch journal. Orca's title is a discovery hint, never an
//! ownership key. A creation intent precedes the external call; its shell
//! acknowledges its runtime handle before starting the guarded harness.

use std::path::PathBuf;

use afkconvoy_core::line::Line;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{copies::cache, host_watch::Watch, orca::Worktree};

pub(crate) const TITLE: &str = "afkconvoy watch";
const FILE: &str = "watch/terminal.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Creating,
    Ready,
    Sending,
    Running,
    Ended,
}

/// The snapshot repairs a missing state/ID after a runner crash. Once the
/// state has a running watch, that watch remains authoritative for progress.
#[derive(Serialize, Deserialize)]
pub(crate) struct Record {
    pub watch: Watch,
    pub phase: Phase,
    /// Public Orca metadata from our creation receipt or verified handshake.
    pub terminal: Option<Value>,
    pub problem: Option<String>,
    pub command: String,
}

fn file() -> Result<PathBuf> {
    Ok(cache()?.join(FILE))
}

pub(crate) fn read() -> Result<Option<Record>> {
    let path = file()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("cannot read watch association {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn save(record: &Record) -> Result<()> {
    let path = file()?;
    std::fs::create_dir_all(path.parent().expect("association parent"))?;
    let temporary = path.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(record)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub(crate) fn active() -> Result<Option<Watch>> {
    Ok(read()?.filter(|r| r.phase != Phase::Ended).map(|r| r.watch))
}

fn handshake(watch: &Watch) -> PathBuf {
    watch.transcript.with_extension("terminal")
}

pub(crate) fn claim(watch: &Watch) -> PathBuf {
    watch.transcript.with_extension("claim")
}

pub(crate) fn matches(record: &Record, watch: &Watch) -> bool {
    record.watch.id == watch.id && record.watch.attempts.len() == watch.attempts.len()
}

/// Refuses unknown same-name sessions before creating anything. The host
/// runner's host lock serializes callers; the shell's mkdir also fences a
/// duplicated external create request from running a second harness.
pub(crate) fn prepare(watch: &Watch, command: &str) -> Result<()> {
    ensure!(active()?.is_none(), "a watch launch is still unresolved; no second agent started");
    let worktree = Worktree::at(watch.directory.clone(), String::new());
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
        watch: watch.clone(),
        phase: Phase::Creating,
        terminal: None,
        problem: None,
        command: command.into(),
    })
}

/// Wraps the existing guarded command without interpreting or replacing it.
/// mkdir is intentionally retained after exit: this launch may run once.
/// A missing Orca handle cannot acknowledge ownership and runs no harness.
pub(crate) fn launch_line(watch: &Watch, line: &str) -> String {
    let claim = claim(watch).display().to_string();
    let receipt = handshake(watch).display().to_string();
    let temporary = format!("{receipt}.tmp");
    Line::new()
        .fixed("if mkdir ").word(&claim)
        .fixed("; then if [ -n \"${ORCA_TERMINAL_HANDLE-}\" ]; then printf '%s\\n' \"$ORCA_TERMINAL_HANDLE\" > ")
        .word(&temporary).fixed(" && mv ").word(&temporary).fixed(" ").word(&receipt)
        .fixed(" && { ").launch(line)
        .fixed("; }; else printf '%s\\n' 'watch launch has no ORCA_TERMINAL_HANDLE; recovery required' >&2; fi; else printf '%s\\n' 'watch launch already claimed; no second agent started' >&2; fi")
        .done()
}

pub(crate) fn created(watch: &Watch, handle: &str) -> Result<()> {
    let mut record = read()?.context("watch launch has no intent")?;
    ensure!(matches(&record, watch), "watch launch intent changed");
    // Query public metadata rather than assuming titles or runtime handles
    // survive an Orca restart. Retain the intent if this query fails.
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    let rows = worktree.watch_terminals()?;
    let row = rows
        .into_iter()
        .find(|row| row["handle"] == handle)
        .context("created watch terminal is not listed; recovery required")?;
    record.watch.terminal = handle.into();
    record.terminal = Some(row);
    record.phase = Phase::Ready;
    save(&record)
}

/// Exact saved handle first, then stable public pane identity or the shell's
/// own acknowledgement. Unknown or multiple discovery candidates stay open.
/// None means a previously confirmed terminal is now absent.
pub(crate) fn locate(watch: &Watch) -> Result<Option<String>> {
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    let rows = worktree.watch_terminals()?;
    let record = read()?.filter(|record| matches(record, watch));
    let metadata = record.as_ref().and_then(|r| r.terminal.as_ref());
    let exact = if watch.terminal.is_empty() {
        record.as_ref().map_or("", |r| r.watch.terminal.as_str())
    } else {
        &watch.terminal
    };
    if !exact.is_empty()
        && let Some(row) = rows.iter().find(|row| row["handle"] == exact)
    {
        if let Some(saved) = metadata {
            for key in ["ptyId", "incarnationId"] {
                ensure!(
                    saved[key].is_null() || saved[key] == row[key],
                    "watch terminal {} changed {key}; ownership is unverified",
                    watch.terminal
                );
            }
        }
        return Ok(Some(exact.to_string()));
    }
    let acknowledged = std::fs::read_to_string(handshake(watch)).ok();
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
        if !watch.terminal.is_empty() {
            return Ok(None);
        }
        bail!("watch creation has no receipt or confirmed terminal; creation will not be repeated");
    }
    ensure!(
        candidates.len() == 1,
        "multiple watch terminal candidates: {}; recovery required",
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
    let handle = row["handle"].as_str().context("watch terminal has no handle")?.to_string();
    if let Some(mut record) = record {
        record.watch.terminal = handle.clone();
        record.terminal = Some(row.clone());
        if record.phase == Phase::Creating {
            record.phase = Phase::Ready;
        }
        save(&record)?;
    }
    Ok(Some(handle))
}

pub(crate) fn phase(watch: &Watch, phase: Phase) -> Result<()> {
    if let Some(mut record) = read()?.filter(|r| matches(r, watch)) {
        record.watch = watch.clone();
        record.phase = phase;
        record.problem = None;
        save(&record)?;
    }
    Ok(())
}

pub(crate) fn problem(why: &str) -> Result<()> {
    if let Some(mut record) = read()? {
        record.problem = Some(why.into());
        save(&record)?;
    }
    Ok(())
}

pub(crate) fn recovery(why: &str) -> String {
    format!(
        "{why}. No replacement is started and existing work is retained. Run `afkconvoy doctor --all` and `orca terminal list --worktree path:<watch-directory> --json`; inspect candidates with `orca terminal show --terminal <handle> --json`. Rename unrelated same-name terminals with `orca terminal rename --terminal <handle> --title <title> --json`, then let the runner retry. For an uncertain launch or prompt, inspect the existing session and submit its complete report using `afkconvoy watched --watch <id> --text-file <report.json>`; this ends only its marked executor. Keep the brief, transcript and launch claim; do not resend creation or the prompt."
    )
}

pub(crate) fn json() -> Value {
    match read() {
        Ok(Some(record)) => json!({"title": TITLE, "watch": record.watch.id,
            "directory": record.watch.directory, "terminal": record.watch.terminal,
            "identity": record.terminal, "phase": record.phase, "problem": record.problem}),
        Ok(None) => json!({"title": TITLE, "phase": "unassociated"}),
        Err(error) => {
            json!({"title": TITLE, "phase": "uncertain", "problem": recovery(&format!("{error:#}"))})
        }
    }
}

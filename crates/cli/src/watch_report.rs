//! The watch's explicit report. A separate, atomically published file lets
//! the command report without racing the host runner's watch state writes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Human {
    /// The condition id from this round's look. Older reports may omit it
    /// only when that look contains exactly one actionable condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    pub question: String,
    pub answer: String,
}

/// Resolves by the fleet's structured identities, never by a question or
/// answer's words. Ambiguous reports must name their condition explicitly.
pub fn condition(watch: &crate::host_watch::Watch, human: &Human) -> Result<String> {
    let items = crate::host_watch::actionable(&watch.look);
    match &human.condition {
        Some(id) if items.iter().any(|item| &item.id == id) => Ok(id.clone()),
        None if items.len() == 1 => Ok(items[0].id.clone()),
        _ => bail!(
            "each human entry must name a `condition` from this round's look; choose one of: {}",
            serde_json::to_string(&items.iter().map(|item| &item.id).collect::<Vec<_>>())?
        ),
    }
}

/// The issue's source link in the look; host conditions have no issue URL.
pub fn issue_url(watch: &crate::host_watch::Watch, condition: &str) -> Option<String> {
    for checkout in watch.look["checkouts"].as_array().into_iter().flatten() {
        let Some(at) = checkout["checkout"].as_str() else { continue };
        for waiting in checkout["doctor"]["waiting_on_human"].as_array().into_iter().flatten() {
            let Ok(id) = crate::doctor::waiting_id(at, waiting) else { continue };
            if id == condition {
                return waiting["url"].as_str().map(str::to_string);
            }
        }
    }
    crate::unworked::issue_url(&watch.look["host"]["unworked_repos"], condition)
}

/// All five parts are required, including empty lists: absence must never
/// imply that a watch checked something or left nothing pending.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub held_back: Vec<String>,
    pub did: Vec<String>,
    pub verified: Vec<String>,
    pub human: Vec<Human>,
    pub pending: Vec<String>,
}

pub fn directory(id: &str) -> Result<PathBuf> {
    // Ids also arrive from a command's argument, never a path supplied by it.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        bail!("invalid watch id; use the `afkconvoy watched` line in the watch's brief");
    }
    Ok(crate::copies::cache()?.join("watch").join(id))
}

pub fn read(id: &str) -> Result<Option<Report>> {
    match std::fs::read(directory(id)?.join("reported.json")) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Records once for the running watch. Repeating its exact report succeeds;
/// replacing it or reporting for an ended/different watch is refused.
pub fn record(id: &str, file: &Path, now: u64) -> Result<Value> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let report: Report = serde_json::from_slice(&bytes).with_context(|| {
        format!("{}: fix the report JSON; see `afkconvoy watched --help`", file.display())
    })?;
    if report
        .held_back
        .iter()
        .chain(&report.did)
        .chain(&report.verified)
        .chain(&report.pending)
        .map(String::as_str)
        .chain(
            report.human.iter().flat_map(|human| [human.question.as_str(), human.answer.as_str()]),
        )
        .any(|text| text.trim().is_empty())
    {
        bail!("report entries must be nonempty; use [] for a part with nothing to report");
    }
    let directory = directory(id)?;
    std::fs::create_dir_all(&directory)?;
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("report.lock"))?;
    held.lock()?;
    let _state_lock = crate::host_watch::state_lock()?;
    // A look or another report may have held the lock. Validate the hour
    // when the transition can occur, not when this command began waiting.
    let now = now.max(crate::clock());
    if let Some(kept) = read(id)? {
        if serde_json::to_value(&kept)? != serde_json::to_value(&report)? {
            bail!("watch {id} already reported; its record cannot be replaced");
        }
    } else {
        let watch =
            crate::host_watch::running()?.context("no watch is running; nothing recorded")?;
        if watch.id != id
            || (watch.started != 0 && afkconvoy_core::host_watch::expired(watch.started, now))
        {
            bail!("watch {id} is not the running watch within its hour; nothing recorded");
        }
        for human in &report.human {
            condition(&watch, human)?;
        }
        let temporary = directory.join(format!("report-{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&report)?)?;
        std::fs::rename(temporary, directory.join("reported.json"))?;
    }
    Ok(json!({"watch": id, "reported": true, "report": report}))
}

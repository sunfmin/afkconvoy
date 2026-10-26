//! The watch's explicit report. A separate, atomically published file lets
//! the command report without racing the host runner's watch state writes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Human {
    pub question: String,
    pub answer: String,
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
    if let Some(kept) = read(id)? {
        if serde_json::to_value(&kept)? != serde_json::to_value(&report)? {
            bail!("watch {id} already reported; its record cannot be replaced");
        }
    } else {
        let watch =
            crate::host_watch::running()?.context("no watch is running; nothing recorded")?;
        if watch.id != id || afkconvoy_core::host_watch::expired(watch.started, now) {
            bail!("watch {id} is not the running watch within its hour; nothing recorded");
        }
        let temporary = directory.join(format!("report-{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&report)?)?;
        std::fs::rename(temporary, directory.join("reported.json"))?;
    }
    Ok(json!({"watch": id, "reported": true, "report": report}))
}

//! A question's explicit settlement, published atomically for the asking
//! scheduler. A settled answer links the operator's actual words on its issue.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Settled,
    Aside,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn directory(id: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        bail!("invalid question id; use the `afkconvoy asked` line in the asking session's brief");
    }
    Ok(crate::copies::cache()?.join("asking/reports").join(id))
}

pub(crate) fn report_file(id: &str) -> Result<PathBuf> {
    Ok(directory(id)?.join("report.json"))
}

pub(crate) fn usage_file(id: &str) -> Result<PathBuf> {
    Ok(directory(id)?.join("usage.json"))
}

pub fn read(id: &str) -> Result<Option<Report>> {
    match std::fs::read(directory(id)?.join("reported.json")) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Records once for the current delivered question. Repeating the exact report
/// succeeds; replacing it or reporting another question is refused.
pub fn record(id: &str, file: &Path) -> Result<Value> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let report: Report = serde_json::from_slice(&bytes).with_context(|| {
        format!("{}: fix the report JSON; see `afkconvoy asked --help`", file.display())
    })?;
    match report.outcome {
        Outcome::Settled
            if !report
                .url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://")) =>
        {
            bail!(
                "a settled question needs `url`: the issue comment holding the operator's quoted words and their time"
            )
        }
        Outcome::Aside if report.url.is_some() => {
            bail!("a question put aside has no settlement URL; remove `url`")
        }
        _ => {}
    }
    let directory = directory(id)?;
    std::fs::create_dir_all(&directory)?;
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("report.lock"))?;
    held.lock()?;
    let _state_lock = crate::host_asking::state_lock()?;
    if let Some(kept) = read(id)? {
        if serde_json::to_value(&kept)? != serde_json::to_value(&report)? {
            bail!("question {id} already reported; its record cannot be replaced");
        }
    } else {
        crate::host_asking::current()?
            .filter(|current| current.question.id == id && current.delivered && !current.withdrawn)
            .context(
                "no such delivered question is current; use the id in the asking session's brief",
            )?;
        let temporary = directory.join(format!("report-{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&report)?)?;
        std::fs::rename(temporary, directory.join("reported.json"))?;
    }
    Ok(json!({"question": id, "reported": true, "report": report}))
}

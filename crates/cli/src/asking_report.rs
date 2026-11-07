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

/// What `afkconvoy asked --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

After the operator has decided the current question, or asked to put it aside, using the exact line in the asking session's brief:

```
afkconvoy asked --question <episode-id> --text-file <report.json>
```

A settled report is {"outcome":"settled","url":"<comment-url>"}. Its URL is the issue comment holding the operator's quoted words and their time; the asking session's own words establish no human intent. A put-aside report is {"outcome":"aside"} and has no URL. Put-aside questions stay pending and are listed when the operator asks.

## Who runs it

The resident asking session, after recording the operator's actual decision on the issue. The operator may repeat its exact command if the receipt was lost.

## What it reads and writes

It reads the current delivered question on this host and atomically keeps the report in the host cache. It needs no repo or GitHub login. Only the current, delivered, unwithdrawn question may report. The next question is delivered only after this report. The same report may be submitted again; a different one cannot replace it.

## What it answers

{question, reported: true, report}: the question's episode identity and its record. Errors name the current question's brief and the JSON to fix.

## What trips you up

Settling requires the link to the operator's quoted words and time, not a summary written by the Agent. Printed output is no report. A recovered question is withdrawn by the host runner and cannot be settled afterwards."#;

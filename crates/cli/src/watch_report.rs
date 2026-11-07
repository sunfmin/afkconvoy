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
            let Ok(identity) = serde_json::from_value::<crate::doctor::HumanWait>(waiting.clone())
            else {
                continue;
            };
            if crate::host_watch::scoped_finding_id(at, &identity.watch_id()) == condition {
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

/// What `afkconvoy watched --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

As the watch's last step, using the exact line at the end of its brief:

```
afkconvoy watched --watch <id> --text-file <report.json>
```

Write a JSON object with all five keys: `held_back`, `did`, `verified` and `pending` are lists of nonempty strings; `human` is a list of {condition, question, answer}, all nonempty strings. `condition` names an exact condition id from the round's look, listed in its brief, and is checked against that look. It may be omitted only for a look with exactly one actionable condition; ambiguous or unknown conditions are refused. Question and answer words never identify a condition. Use [] where nothing belongs. `held_back` says what held work back; `did` each action and its evidence, including repo, PR, cut and version; `verified` what was checked and what it showed; `human` each decision left alone and its exact answer command; `pending` everything unfinished. Printed terminal output is no report.

## Who runs it

The host's watch, in its watch directory, as the last step of each round in its resident interactive session. The operator may repeat the watch's exact command if its receipt was lost.

## What it reads and writes

It reads the running watch on this host and the report file, and atomically keeps the report in the host cache. It needs no repo or GitHub login. The same report may be submitted again; a different report cannot replace it. A missing part, unknown key, empty entry, wrong watch or expired hour is refused with {error} and nonzero exit; fix the JSON or use the running watch's brief.

Once the host runner reads the report, it appends one entry for the round to the daybook (`afkconvoy daybook --help`), of session kind `watch`: what held work back under `Why`; the actions, what was verified and what was left under `Did`; and the issues and PRs the report links under `Links`. Each human question still goes to the asking session; the entry notes only that it was raised, with its link, and asks nothing. The entry is written later and retried until it is: it never holds the round, or the next one, back; one not written is a `doctor` finding.

Recording completes only the named round and keeps the healthy resident Agent and Terminal available for later rounds. It does not finish queued work or another active round. `doctor.host.watch` and later briefs show the report; an ended watch with no report says `reported: false`.

## What it answers

{watch, reported: true, report}: the watch id and its complete record. Reporting does not claim recovery beyond the evidence in the report.

## What trips you up

Use the id and report path from this watch's brief. Include all five parts, even those with empty lists. The report cannot be replaced once recorded, and a successful harness exit without this command is shown as not reported."#;

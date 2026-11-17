//! Token usage of patrol workers, retained before their worktree is removed.

use afkconvoy_core::accounts::Usage;
use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::orca::Worktree;

#[derive(Serialize, Deserialize)]
struct Record {
    repo: String,
    #[serde(flatten)]
    usage: Usage,
}

/// The patrol runs' usage in the host's database: a row each of
/// `patrol_runs`, by its id. It was `accounts/patrol-runs.json` in the
/// cache directory, with `accounts/patrol-runs.lock` beside it.
pub(crate) static STORE: crate::host_db::Store = crate::host_db::Store {
    file: "accounts/patrol-runs.json",
    old: crate::host_db::Old::Json,
    layout: crate::host_db::Layout::Rows("patrol_runs", |row| {
        row["id"].as_str().unwrap_or("").into()
    }),
};

fn read() -> Result<Vec<Record>> {
    crate::host_db::read(&STORE).context("cannot read patrol usage")
}

/// Keeps the usage of the patrol run `patrol` of `repo` its worker in
/// `checkout` made from `started` to `ended`, in place of what was kept of
/// that run: read from its sessions first, then written in one transaction.
pub fn record(
    repo: &str,
    checkout: &Worktree,
    patrol: &str,
    started: u64,
    ended: u64,
) -> Result<()> {
    let current = checkout.harness()?;
    let model = std::fs::read_to_string(checkout.note("convoy-worker-model")?)
        .ok()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty());
    let brief = checkout.note("convoy-worker-prompt.md")?;
    let brief_root = brief.parent().context("patrol brief has no directory")?;
    let prefix = format!("{repo}:patrol:{patrol}:{started}:");
    let mut rows = vec![];
    for harness in Harness::ALL {
        let tokens =
            crate::session::tokens_between(&checkout.path, harness, started, ended, brief_root)
                .ok()
                .flatten();
        if tokens.is_none() && current != Some(harness) {
            continue;
        }
        rows.push(Record {
            repo: repo.to_string(),
            usage: Usage {
                id: format!("{prefix}{}", harness.key()),
                harness: Some(harness.into()),
                model: (current == Some(harness)).then(|| model.clone()).flatten(),
                start: Some(started),
                end: Some(ended),
                seconds: ended.checked_sub(started),
                tokens,
                conditions: vec![format!("patrol:{patrol}")],
            },
        });
    }
    crate::host_db::change(&STORE, |kept: &mut Vec<Record>| {
        kept.retain(|row| !row.usage.id.starts_with(&prefix));
        kept.extend(rows);
    })
}

pub fn runs(repo: &str) -> Result<Vec<Usage>> {
    Ok(read()?.into_iter().filter(|row| row.repo == repo).map(|row| row.usage).collect())
}

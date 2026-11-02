//! Token usage of patrol workers, retained before their worktree is removed.

use std::path::PathBuf;

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

fn file() -> Result<PathBuf> {
    Ok(crate::copies::cache()?.join("accounts/patrol-runs.json"))
}

fn read() -> Result<Vec<Record>> {
    let path = file()?;
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("cannot read patrol usage {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

pub fn record(
    repo: &str,
    checkout: &Worktree,
    patrol: &str,
    started: u64,
    ended: u64,
) -> Result<()> {
    let path = file()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let current = checkout.harness()?;
    let model = std::fs::read_to_string(checkout.note("convoy-worker-model")?)
        .ok()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty());
    let brief = checkout.note("convoy-worker-prompt.md")?;
    let brief_root = brief.parent().context("patrol brief has no directory")?;
    let prefix = format!("{repo}:patrol:{patrol}:{started}:");
    let mut rows: Vec<Record> =
        read()?.into_iter().filter(|row| !row.usage.id.starts_with(&prefix)).collect();
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
                harness: Some(harness.key().to_string()),
                model: (current == Some(harness)).then(|| model.clone()).flatten(),
                start: Some(started),
                end: Some(ended),
                seconds: ended.checked_sub(started),
                tokens,
                conditions: vec![format!("patrol:{patrol}")],
            },
        });
    }
    let temporary = path.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&rows)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub fn runs(repo: &str) -> Result<Vec<Usage>> {
    Ok(read()?.into_iter().filter(|row| row.repo == repo).map(|row| row.usage).collect())
}

//! One-time conversion of the tracker's old work accounts.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use afkconvoy_core::{account, accounts, record, tracker};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{checkout_list, gh::GitHub, git::Repo};

#[derive(Serialize)]
struct Change {
    issue: u64,
    comment: u64,
    old_body: String,
    new_body: Option<String>,
    check: String,
}

/// Plan from a fresh tracker read; in write mode, save and sync each original
/// before checking that the remote body still matches and editing it.
pub fn run(repo: &Repo, backup: Option<&Path>, now: u64) -> Result<Value> {
    let tracker = checkout_list::tracker_of(repo)?;
    let gh = GitHub::new(repo.dir.clone(), tracker.clone());
    let issues = gh.account_issues()?;
    let mut changes = Vec::new();
    let mut before = Vec::new();
    let mut after = Vec::new();
    for issue in issues {
        let mut bodies = Vec::new();
        for (index, comment) in issue.comments.iter().enumerate() {
            if record::read_marker(account::ACCOUNT, &comment.body).is_none() {
                continue;
            }
            if comment.body.contains("```json\n") {
                bodies.push((comment.body.clone(), comment.body.clone()));
                continue;
            }
            let previous = account::rewrite::legacy(&comment.body, issue.number)
                .map(|row| account::render(&row))
                .unwrap_or_else(|_| comment.body.clone());
            let converted =
                account::rewrite::convert(&comment.body, issue.number).map(|mut row| {
                    if issue.closed && issue.landed {
                        row.outcome = Some("landed".into());
                    } else if issue.comments[..index].last().is_some_and(|note| {
                        record::read_marker("convoy:escalation", &note.body).is_some()
                    }) {
                        row.outcome = Some("escalated".into());
                    }
                    row
                });
            let (new_body, check) = match converted {
                Ok(row) => (Some(account::render(&row)), "pass".to_string()),
                Err(reason) => (None, reason),
            };
            let projected = new_body.clone().unwrap_or_else(|| comment.body.clone());
            bodies.push((previous, projected));
            changes.push(Change {
                issue: issue.number,
                comment: comment.id,
                old_body: comment.body.clone(),
                new_body,
                check,
            });
        }
        if !tracker.holds(&issue.labels) || tracker::repo_labels(&issue.labels).len() > 1 {
            continue;
        }
        let make = |post: bool| accounts::Issue {
            number: issue.number,
            closed: issue.closed,
            url: issue.url.clone(),
            account: match bodies.as_slice() {
                [] => Ok(None),
                [(old, new)] => Ok(Some(if post { new.clone() } else { old.clone() })),
                _ => Err("issue has several work accounts".into()),
            },
        };
        before.push(make(false));
        after.push(make(true));
    }
    let before = serde_json::to_value(accounts::summarize(before, None, now))?;
    let after = serde_json::to_value(accounts::summarize(after, None, now))?;
    if let Some(dir) = backup {
        fs::create_dir_all(dir)
            .with_context(|| format!("create backup directory {}", dir.display()))?;
        for change in &changes {
            let Some(new_body) = &change.new_body else { continue };
            let path = dir.join(format!("{}-{}.json", change.issue, change.comment));
            let data = serde_json::to_vec(
                &json!({"issue":change.issue,"comment":change.comment,"body":change.old_body}),
            )?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(&data)?;
                    file.sync_all()?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if fs::read(&path)? != data {
                        bail!("backup {} already exists with different contents", path.display());
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("write backup {}", path.display()));
                }
            }
            let comments = gh.comments(change.issue)?;
            let current = comments
                .iter()
                .find(|comment| comment.id == change.comment)
                .context("account comment disappeared")?;
            if current.body != change.old_body {
                bail!(
                    "issue {} comment {} changed after the read; run again",
                    change.issue,
                    change.comment
                );
            }
            gh.edit_comment(change.comment, new_body)?;
        }
    }
    Ok(
        json!({"dry_run": backup.is_none(), "backup": backup.map(|p| p.display().to_string()), "rewrite": changes.iter().filter(|c| c.new_body.is_some()).count(), "left": changes.iter().filter(|c| c.new_body.is_none()).count(), "changes":changes, "before":before, "after":after}),
    )
}

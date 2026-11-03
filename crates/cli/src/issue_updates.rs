//! Later words on an open issue reach its current worker through tell's
//! session proof. GitHub remains their home. The worktree keeps only the
//! body last observed and delivery receipts, scoped to this worker's start.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::gh::OpenIssues;
use afkconvoy_core::harness::Harness;

use crate::orca::{Handed, Worktree};

const FILE: &str = "convoy-issue-updates.json";

#[derive(Serialize, Deserialize)]
struct Seen {
    started: u64,
    body: String,
    revision: u64,
    pending: Vec<Update>,
    told: BTreeSet<String>,
    #[serde(default)]
    ignored: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Update {
    mark: String,
    text: String,
    receipt: String,
    /// The tick that first found its line taken by the terminal but not yet
    /// given to the model. It may stay queued for `note_wait_seconds`.
    #[serde(default)]
    queued_at: Option<u64>,
}

impl Seen {
    fn new(started: u64, body: &str) -> Self {
        Self {
            started,
            body: body.into(),
            revision: 0,
            pending: vec![],
            told: BTreeSet::new(),
            ignored: BTreeSet::new(),
        }
    }

    fn save(&self, worktree: &Worktree) -> Result<()> {
        let file = worktree.note(FILE)?;
        let temporary = file.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(self)?)?;
        std::fs::rename(temporary, file)?;
        Ok(())
    }
}

/// Capture the body before the worker takes its brief. An incomplete start
/// can repeat this; a new worker always receives a new baseline.
pub(crate) fn starting(worktree: &Worktree, body: &str) -> Result<()> {
    Seen::new(worktree.handed(None)?.unwrap_or(0), body).save(worktree)
}

/// Tell later comments and body changes before the tick judges or advances
/// a worker. Each result is a tick step; a failed delivery costs only itself.
pub fn reconcile(fleet: &Fleet, host: &str, issues: &OpenIssues) -> Vec<Value> {
    let mut steps = vec![];
    for issue in &issues.rows {
        // Only this fleet's repo and open issues enter OpenIssues. Closed
        // issues, PR comments and claims on other hosts never get typed.
        let number = issue.number;
        let made = (|| -> Result<Vec<Value>> {
            let Ok((worktree, terminal)) = crate::tell::worker(fleet, host, number)? else {
                return Ok(vec![]);
            };
            let Some(started) = worktree.handed(None)? else { return Ok(vec![]) };
            let Some(body) = issues.body(number) else { return Ok(vec![]) };
            let file = worktree.note(FILE)?;
            let mut unknown_body = false;
            let mut seen: Seen = match std::fs::read(&file) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .with_context(|| format!("cannot read {}", file.display()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    unknown_body = true;
                    Seen::new(started, &body.body)
                }
                Err(error) => return Err(error.into()),
            };
            if seen.started != started {
                unknown_body = true;
                seen = Seen::new(started, &body.body);
            }
            if seen.body != body.body
                || unknown_body && issues.body_edited(number).is_some_and(|edited| edited > started)
            {
                seen.revision += 1;
                let mark = format!("{}#convoy-body-{started}-{}", body.url, seen.revision);
                seen.pending.push(Update {
                    mark,
                    text: format!(
                        "Issue body edited ({}). {}",
                        body.url,
                        body_change((!unknown_body).then_some(seen.body.as_str()), &body.body)
                    ),
                    receipt: format!("convoy-body-send-{started}-{}", seen.revision),
                    queued_at: None,
                });
                seen.body.clone_from(&body.body);
            }
            let harness = worktree.recorded_harness()?.unwrap_or(Harness::DEFAULT);
            for comment in issues.comments(number) {
                if comment.at.is_none_or(|at| at <= started)
                    || comment.body.contains("<!--convoy:")
                    || seen.told.contains(&comment.url)
                    || seen.ignored.contains(&comment.url)
                    || seen.pending.iter().any(|update| update.mark == comment.url)
                {
                    continue;
                }
                if crate::session::posted_comment(&worktree.path, harness, started, &comment.url)? {
                    seen.ignored.insert(comment.url.clone());
                    continue;
                }
                seen.pending.push(Update {
                    mark: comment.url.clone(),
                    text: format!("New issue comment ({}): {}", comment.url, comment.body),
                    receipt: format!("convoy-comment-send-{started}-{}", comment.id),
                    queued_at: None,
                });
            }
            // Persist before delivery: a body that changes again while its
            // earlier revision waits does not erase the earlier instruction.
            seen.save(&worktree)?;
            let mut made = vec![];
            let pending = std::mem::take(&mut seen.pending);
            for mut update in pending {
                let line = format!(
                    "{} [issue update: {}]",
                    crate::tell::one_line(&update.text),
                    update.mark
                );
                let told = worktree.hand_update(
                    &terminal,
                    &line,
                    &update.mark,
                    started,
                    &worktree.note(&update.receipt)?,
                );
                let told = match told {
                    Ok(Handed::Queued) => queued(&mut update, fleet.now, harness),
                    Ok(handed) => worktree.record_told(fleet.now, &update.mark).map(|()| handed),
                    Err(error) => Err(error),
                };
                let mut step = json!({"do": "tell", "issue": number, "comment": update.mark});
                match told {
                    Ok(Handed::Queued) => {
                        step["outcome"] = json!("queued");
                        step["since"] = json!(update.queued_at);
                        seen.pending.push(update);
                    }
                    Ok(handed) => {
                        step["outcome"] = json!("done");
                        if let Handed::Taken { seconds } = handed {
                            step["seconds"] = json!(seconds);
                        } else {
                            step["typed"] = json!(false);
                        }
                        seen.told.insert(update.mark);
                    }
                    Err(error) => {
                        step["outcome"] = json!("failed");
                        step["error"] = json!(format!("{}: {error:#}", update.mark));
                        seen.pending.push(update);
                    }
                }
                made.push(step);
            }
            seen.save(&worktree)?;
            Ok(made)
        })();
        match made {
            Ok(made) => steps.extend(made),
            Err(error) => steps.push(json!({"do": "tell", "issue": number,
                "outcome": "failed", "error": format!("{error:#}")})),
        }
    }
    steps
}

/// A line the terminal took waits in its harness's input until the step the
/// harness is on ends. It stays queued, no failure, until the bound a typed
/// note has to reach the model; past it, it fails each tick it is still not
/// taken, and no second line is typed.
fn queued(update: &mut Update, now: u64, harness: Harness) -> Result<Handed> {
    let since = *update.queued_at.get_or_insert(now);
    let bound = crate::orca::note_wait_seconds();
    if now.saturating_sub(since) < bound {
        return Ok(Handed::Queued);
    }
    anyhow::bail!(
        "{} did not give its model the line queued since {since} within {bound}s: it is not \
         delivered, and no second line was typed",
        harness.name()
    )
}

/// A replacement block drops the common beginning and end. Without a
/// meaningful baseline, or for whitespace-only edits, keep the new body.
fn body_change(before: Option<&str>, after: &str) -> String {
    if let Some(before) = before.filter(|before| !before.is_empty()) {
        let old: Vec<&str> = before.lines().collect();
        let new: Vec<&str> = after.lines().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let old = &old[prefix..];
        let new = &new[prefix..];
        let suffix = old.iter().rev().zip(new.iter().rev()).take_while(|(a, b)| a == b).count();
        let removed = &old[..old.len() - suffix];
        let added = &new[..new.len() - suffix];
        if !removed.is_empty() || !added.is_empty() {
            let lines: Vec<String> = removed
                .iter()
                .map(|line| format!("- {line}"))
                .chain(added.iter().map(|line| format!("+ {line}")))
                .collect();
            return format!(
                "Changed lines (replace the removed block with the added block):\n{}",
                lines.join("\n")
            );
        }
    }
    format!("Whole new body: {after}")
}

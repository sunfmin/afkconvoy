//! Later words on an open issue reach its current worker through tell's
//! session proof. GitHub remains their home. The worktree keeps only the
//! body last observed and delivery receipts, scoped to this worker's start.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::gh::OpenIssues;
use crate::orca::{Handed, Worktree};

const FILE: &str = "convoy-issue-updates.json";

#[derive(Serialize, Deserialize)]
struct Seen {
    started: u64,
    body: String,
    revision: u64,
    pending: Vec<Update>,
    told: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Update {
    mark: String,
    text: String,
    receipt: String,
}

impl Seen {
    fn new(started: u64, body: &str) -> Self {
        Self { started, body: body.into(), revision: 0, pending: vec![], told: BTreeSet::new() }
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
pub(crate) fn reconcile(fleet: &Fleet, host: &str, issues: &OpenIssues) -> Vec<Value> {
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
            let mut seen: Seen = match std::fs::read(&file) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .with_context(|| format!("cannot read {}", file.display()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Seen::new(started, &body.body)
                }
                Err(error) => return Err(error.into()),
            };
            if seen.started != started {
                seen = Seen::new(started, &body.body);
            }
            if seen.body != body.body {
                seen.revision += 1;
                let mark = format!("{}#convoy-body-{started}-{}", body.url, seen.revision);
                // Keep the full new body: a worker can compare it with the
                // version it read; no diff library or shell quoting involved.
                seen.pending.push(Update {
                    mark,
                    text: format!(
                        "Issue body edited ({}). Whole new body: {}",
                        body.url, body.body
                    ),
                    receipt: format!("convoy-body-send-{started}-{}", seen.revision),
                });
                seen.body.clone_from(&body.body);
            }
            let harness =
                worktree.recorded_harness()?.unwrap_or(afkconvoy_core::harness::Harness::DEFAULT);
            for comment in issues.comments(number) {
                if comment.at.is_none_or(|at| at <= started)
                    || comment.body.contains("<!--convoy:")
                    || seen.told.contains(&comment.url)
                    || seen.pending.iter().any(|update| update.mark == comment.url)
                    || crate::session::posted_comment(
                        &worktree.path,
                        harness,
                        started,
                        &comment.url,
                    )?
                {
                    continue;
                }
                seen.pending.push(Update {
                    mark: comment.url.clone(),
                    text: format!("New issue comment ({}): {}", comment.url, comment.body),
                    receipt: format!("convoy-comment-send-{started}-{}", comment.id),
                });
            }
            // Persist before delivery: a body that changes again while its
            // earlier revision waits does not erase the earlier instruction.
            seen.save(&worktree)?;
            let mut made = vec![];
            let pending = std::mem::take(&mut seen.pending);
            for update in pending {
                let line = format!(
                    "{} [issue update: {}]",
                    crate::tell::one_line(&update.text),
                    update.mark
                );
                let told = worktree
                    .hand_update(
                        &terminal,
                        &line,
                        &update.mark,
                        started,
                        &worktree.note(&update.receipt)?,
                    )
                    .and_then(|handed| {
                        worktree.record_told(fleet.now, &update.mark)?;
                        Ok(handed)
                    });
                let mut step = json!({"do": "tell", "issue": number, "comment": update.mark});
                match told {
                    Ok(handed) => {
                        step["outcome"] = json!("done");
                        match handed {
                            Handed::Taken { seconds } => step["seconds"] = json!(seconds),
                            Handed::Already => step["typed"] = json!(false),
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

#[cfg(test)]
mod tests {
    /// Opt-in live acceptance check: the named issue must already be claimed
    /// on this host in this worktree. Only its new, read-only smoke worker is
    /// closed; ordinary workers and every other issue are left alone.
    #[test]
    #[ignore = "starts real Claude in Orca and posts one issue comment; AFKCONVOY_LIVE_ISSUE=N"]
    fn a_real_claimed_worker_receives_a_later_comment_in_its_session() {
        use super::*;
        use crate::orca::orca;
        use afkconvoy_core::harness::{Harness, SessionLayout};
        use std::time::{Duration, Instant};

        struct Terminal(String);
        impl Drop for Terminal {
            fn drop(&mut self) {
                let _ = orca(&["terminal", "close", "--terminal", &self.0]);
            }
        }

        let issue: u64 =
            std::env::var("AFKCONVOY_LIVE_ISSUE").expect("name the claimed issue").parse().unwrap();
        let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let repo = crate::git::Repo::new(checkout.clone());
        let fleet = Fleet::open(&repo, crate::clock()).unwrap();
        let host = crate::this_host();
        assert!(
            crate::tell::worker(&fleet, &host, issue).unwrap().is_ok(),
            "a local claimed worker is required"
        );
        let selector = format!("path:{}", checkout.display());
        let launch = orca(&["agent", "launch", "--agent", "claude", "--worktree", &selector,
            "--surface", "terminal", "--prompt",
            "You are a read-only worker in an issue-delivery smoke test. Use no tools and change no files. Reply READY now. On a later issue comment, reply RECEIVED and nothing else."]).unwrap();
        let outcome = launch.get("outcome").unwrap_or(&launch["launch"]["outcome"]);
        assert_eq!(outcome["kind"], "terminal", "{launch}");
        let terminal =
            Terminal(outcome["handle"].as_str().expect("launched terminal handle").into());
        let waited = orca(&[
            "terminal",
            "wait",
            "--terminal",
            &terminal.0,
            "--for",
            "tui-idle",
            "--timeout-ms",
            "60000",
        ])
        .unwrap();
        assert_eq!(waited["wait"]["satisfied"], true, "{waited}");
        let text = format!(
            "Read-only delivery acceptance check {}: reply RECEIVED. This comment requires no code change.",
            uuid::Uuid::new_v4()
        );
        fleet.gh.comment(issue, &text).unwrap();
        let comment = fleet
            .gh
            .comments(issue)
            .unwrap()
            .into_iter()
            .find(|comment| comment.body == text)
            .unwrap();
        let began = Instant::now();
        loop {
            let mut open = fleet.gh.open().unwrap();
            open.issues.rows.retain(|row| row.number == issue);
            let steps = reconcile(&fleet, &host, &open.issues);
            if steps.iter().any(|step| step["comment"] == comment.url && step["outcome"] == "done")
            {
                break;
            }
            assert!(began.elapsed() < Duration::from_secs(90), "{steps:?}");
            std::thread::sleep(Duration::from_millis(250));
        }
        // Read the actual user entry's timestamp, not the terminal receipt or
        // this test's observation clock.
        let place = Harness::ClaudeCode.note_place(&checkout.to_string_lossy());
        let SessionLayout::Files { dir, .. } = place.layout else { unreachable!() };
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let dir = home.join(place.under_home).join(dir);
        let mut entry_at = None;
        for file in std::fs::read_dir(dir).unwrap().flatten() {
            if file.path().extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            for line in std::fs::read_to_string(file.path()).unwrap().lines() {
                if Harness::ClaudeCode.holds_note(line, &comment.url) {
                    let entry: Value = serde_json::from_str(line).unwrap();
                    entry_at = entry["timestamp"]
                        .as_str()
                        .map(|at| crate::doctor::watching::since(at).unwrap());
                }
            }
        }
        let entry_at = entry_at.expect("the real session's user entry");
        let evidence = json!({"issue":issue,"comment":comment.url,"comment_at":comment.at,
            "session_entry_at":entry_at,"seconds":entry_at.saturating_sub(comment.at.unwrap()),
            "terminal":terminal.0,"harness":"claude-code"});
        std::fs::create_dir_all(checkout.join(".afkconvoy-tmp")).unwrap();
        std::fs::write(
            checkout.join(".afkconvoy-tmp/issue-update-live-evidence.json"),
            evidence.to_string(),
        )
        .unwrap();
        println!("{evidence}");
    }
}

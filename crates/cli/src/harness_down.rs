//! Host-local evidence that prompts cannot reach one harness. Claims and work
//! stay where they are; only a successful send clears the evidence.

use std::path::Path;

use afkconvoy_core::harness::Harness;
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::git::Repo;
use crate::orca::Worktree;

const FAILURES: usize = 3;
const PROBE_SECONDS: u64 = 300;

#[derive(Debug)]
pub struct Held;
impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the harness is down; its claims are held until a probe succeeds or a switch moves them")
    }
}
impl std::error::Error for Held {}

#[derive(Default, Serialize, Deserialize)]
struct Evidence {
    reason: String,
    failures: usize,
    terminals: Vec<String>,
    worktrees: Vec<std::path::PathBuf>,
    probe_at: u64,
}
impl Evidence {
    fn down(&self) -> bool {
        self.failures >= FAILURES
    }

    fn failed(&mut self, terminal: &str, reason: &str, now: u64) -> bool {
        let was_down = self.down();
        if self.reason != reason && !was_down {
            *self = Self { reason: reason.to_string(), ..Self::default() };
        }
        self.reason = reason.to_string();
        self.failures = self.failures.saturating_add(1);
        if !self.terminals.iter().any(|seen| seen == terminal) {
            self.terminals.push(terminal.to_string());
        }
        // Several workers sharing a refusal is stronger evidence, but a host
        // with only one worker must stop too. Both are bounded by three sends.
        if self.down() {
            self.probe_at = now.saturating_add(PROBE_SECONDS);
        }
        !was_down && self.down()
    }
}

fn file(repo: &Repo, harness: Harness) -> Result<std::path::PathBuf> {
    repo.note(&format!("convoy-harness-down-{}.json", harness.key()))
}
/// Where a harness found down is paused: on this host — but a unit test's
/// own repo, so that no test pauses a harness of whoever runs it.
fn pauses(repo: &Repo) -> Result<crate::paused::Pauses> {
    if cfg!(test) {
        return Ok(crate::paused::Pauses::at(repo.note("convoy-paused.json")?));
    }
    let _ = repo;
    crate::paused::Pauses::here()
}

fn read(path: &Path) -> Result<Evidence> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Evidence::default()),
        Err(e) => Err(e.into()),
    }
}
fn keep(path: &Path, evidence: &Evidence) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(evidence)?)?;
    Ok(std::fs::rename(temporary, path)?)
}

pub fn down(repo: &Repo, harness: Harness, now: u64) -> Result<bool> {
    let evidence = read(&file(repo, harness)?)?;
    Ok(evidence.down() && now < evidence.probe_at)
}

pub fn any_down(repo: &Repo) -> Result<bool> {
    for harness in Harness::ALL {
        if read(&file(repo, harness)?)?.down() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Only workers with refused prompts may probe. The others, including ones
/// already nudged, cannot escalate during the outage. Keeping all refused
/// workers lets another probe if one of them disappears.
pub fn holds(worktree: &Worktree, now: u64) -> Result<bool> {
    let harness = worktree.harness()?.unwrap_or(Harness::UNNAMED);
    let evidence = read(&file(&Repo::new(worktree.path.clone()), harness)?)?;
    Ok(evidence.down() && (now < evidence.probe_at || !evidence.worktrees.contains(&worktree.path)))
}

/// The one send path, including the probe: after five minutes a normal pending
/// prompt is allowed through. It is never replaced with synthetic work. A
/// refusal holds the harness for five more minutes; acceptance clears it.
pub fn tell(
    worktree: &Worktree,
    terminal: &str,
    now: u64,
    send: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let repo = Repo::new(worktree.path.clone());
    let harness = worktree.harness()?.unwrap_or(Harness::UNNAMED);
    let path = file(&repo, harness)?;
    let mut evidence = read(&path)?;
    if evidence.down() && now < evidence.probe_at {
        return Err(Held.into());
    }
    match send() {
        Ok(()) => {
            if evidence.failures > 0 {
                keep(&path, &Evidence::default())?;
            }
            // A prompt it took is a probe it answered.
            if evidence.down() {
                pauses(&repo)?.resume(harness)?;
            }
            Ok(())
        }
        Err(error) => {
            // Only Orca's structured refusal to accept prompts says anything
            // about the harness. A missing terminal or broken Orca does not.
            if let Some(blocked) = error.downcast_ref::<crate::orca::PromptBlocked>() {
                let report = evidence.failed(terminal, &blocked.0, now);
                if !evidence.worktrees.contains(&worktree.path) {
                    evidence.worktrees.push(worktree.path.clone());
                }
                keep(&path, &evidence)?;
                if report {
                    // No worker is started on it until a probe finds it working.
                    pauses(&repo)?.pause(harness, now, &evidence.reason)?;
                    let instance = crate::log::Log::of(&repo)
                        .ok()
                        .and_then(|log| log.read())
                        .and_then(|entries| {
                            entries
                                .into_iter()
                                .rev()
                                .find(|entry| entry.of_run() && !entry.instance.is_empty())
                                .map(|entry| entry.instance)
                        });
                    let why = description(harness, &evidence.reason, instance.as_deref());
                    eprintln!("{why}");
                    let _ = crate::log::Log::of(&repo).and_then(|log| {
                        log.append(&crate::log::Entry {
                            at: now,
                            instance: instance.unwrap_or_default(),
                            host: gethostname::gethostname().to_string_lossy().into_owned(),
                            what: crate::log::What::Command {
                                command: "harness_down".into(),
                                on: Some(harness.key().into()),
                                by: "fleet".into(),
                                outcome: why,
                                failed: false,
                            },
                        })
                    });
                }
                if evidence.down() {
                    return Err(Held.into());
                }
            } else if evidence.down() {
                // A broken transport is no successful probe. Keep the hold
                // and its cadence even when the probe fails another way.
                evidence.probe_at = now.saturating_add(PROBE_SECONDS);
                keep(&path, &evidence)?;
                return Err(Held.into());
            } else if evidence.failures > 0 {
                // The threshold counts consecutive identical refusals.
                keep(&path, &Evidence::default())?;
            }
            Err(error)
        }
    }
}

fn switch_command(harness: Harness, instance: Option<&str>) -> String {
    format!(
        "afkconvoy switch --instance {} --harness {} --all",
        instance.unwrap_or("<id>"),
        if harness == Harness::Codex { "claude-code" } else { "codex" }
    )
}

fn description(harness: Harness, reason: &str, instance: Option<&str>) -> String {
    format!(
        "{} harness is down: {reason}; it is paused on this host, claims are held, prompts stop, and one pending prompt probes every {PROBE_SECONDS}s; switch with `{}`",
        harness.key(),
        switch_command(harness, instance)
    )
}

pub fn findings(repo: &Repo, instance: Option<&str>) -> Result<Vec<crate::doctor::Finding>> {
    let mut found = vec![];
    for harness in Harness::ALL {
        let evidence = read(&file(repo, harness)?)?;
        if evidence.down() {
            found.push(crate::doctor::finding(
                crate::doctor::Kind::HarnessDown,
                format!("{} harness is down", harness.key()),
                description(harness, &evidence.reason, instance),
                switch_command(harness, instance),
            ));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn worker() -> (tempfile::TempDir, Worktree) {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let worktree = Worktree::at(dir.path().to_path_buf(), "test".into());
        worktree.keep_harness(Harness::ClaudeCode).unwrap();
        (dir, worktree)
    }

    #[test]
    fn shared_identical_refusals_report_once_and_success_resumes_sends() {
        let (_dir, worker) = worker();
        let sends = Cell::new(0);
        let repo = Repo::new(worker.path.clone());
        let refuse = || {
            sends.set(sends.get() + 1);
            Err(crate::orca::PromptBlocked("organization disabled subscription access".into())
                .into())
        };
        for terminal in ["one", "two", "three"] {
            assert!(tell(&worker, terminal, 100, refuse).is_err());
        }
        assert_eq!(sends.get(), 3);
        let paused = pauses(&repo).unwrap().of(Harness::ClaudeCode).unwrap().unwrap();
        assert_eq!((paused.since, paused.why.as_str()), (100, "organization disabled subscription access"));
        assert!(holds(&worker, 101).unwrap());
        assert!(!down(&repo, Harness::Codex, 101).unwrap());
        let found = findings(&repo, Some("alpha")).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].evidence.contains("organization disabled subscription access"));
        assert!(found[0].evidence.contains("claims are held"));
        assert!(found[0].fix.contains("afkconvoy switch"));
        assert_eq!(found[0].fix, "afkconvoy switch --instance alpha --harness codex --all");
        for now in 101..400 {
            assert!(tell(&worker, "one", now, refuse).unwrap_err().is::<Held>());
        }
        assert_eq!(sends.get(), 3, "held ticks must not send");
        assert!(tell(&worker, "three", 400, refuse).unwrap_err().is::<Held>());
        assert_eq!(sends.get(), 4, "one probe after five minutes");
        for now in 401..700 {
            assert!(tell(&worker, "two", now, refuse).is_err());
        }
        assert_eq!(sends.get(), 4);
        tell(&worker, "three", 700, || Ok(())).unwrap();
        assert!(findings(&repo, None).unwrap().is_empty());
        assert_eq!(pauses(&repo).unwrap().of(Harness::ClaudeCode).unwrap(), None, "it answered");
        assert!(!holds(&worker, 700).unwrap());
        tell(&worker, "one", 701, || {
            sends.set(sends.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(sends.get(), 5);
    }

    #[test]
    fn only_identical_prompt_refusals_trip_and_only_once_per_episode() {
        let mut evidence = Evidence::default();
        assert!(!evidence.failed("one", "disabled", 1));
        assert!(!evidence.failed("two", "another refusal", 2));
        assert!(!evidence.failed("one", "disabled", 3));
        assert!(!evidence.failed("two", "disabled", 4));
        assert!(evidence.failed("three", "disabled", 5));
        assert!(!evidence.failed("one", "disabled", 305));
        assert!(!evidence.failed("two", "another refusal", 605));
        assert!(evidence.down(), "a changed refusal is no successful probe");
        let (_dir, worker) = worker();
        for _ in 0..10 {
            assert!(
                tell(&worker, "one", 1, || Err(anyhow::anyhow!("terminal disappeared"))).is_err()
            );
        }
        assert!(findings(&Repo::new(worker.path.clone()), None).unwrap().is_empty());
    }

    #[test]
    fn a_transport_failure_resets_suspicion_but_keeps_an_outage_probe_bounded() {
        let (_dir, worker) = worker();
        let blocked = || Err(crate::orca::PromptBlocked("disabled".into()).into());
        let transport = || Err(anyhow::anyhow!("orca unavailable"));
        for now in [1, 2] {
            assert!(tell(&worker, "one", now, blocked).is_err());
        }
        assert!(tell(&worker, "one", 3, transport).is_err());
        for now in [4, 5] {
            assert!(tell(&worker, "one", now, blocked).is_err());
            assert!(!holds(&worker, now).unwrap());
        }
        assert!(tell(&worker, "one", 6, blocked).unwrap_err().is::<Held>());
        assert!(tell(&worker, "one", 306, transport).unwrap_err().is::<Held>());
        assert!(holds(&worker, 605).unwrap());
        assert!(tell(&worker, "one", 605, || panic!("no second probe")).is_err());
        tell(&worker, "one", 606, || Ok(())).unwrap();
        assert!(!holds(&worker, 606).unwrap());
    }
}

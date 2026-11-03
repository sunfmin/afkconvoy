//! The work account an issue shows when work on it ends. The fleet writes the
//! one comment, and edits it at a later end. A patrol, a cut and a watch are
//! not issues, and this is not called for them.

use std::process::Command;

use afkconvoy_core::account::{self, End, Heard, Machine};
use afkconvoy_core::claims::{Claim, claim_ref};
use afkconvoy_core::layout::GATE_KIND;
use anyhow::{Result, bail};

use crate::gh::GitHub;
use crate::git::Repo;
use crate::remote::Fresh;

/// Leaves the work account on `issue` for its `end` at `now`, or edits the
/// one already there: the one place the fleet writes it. An end that leaves
/// none (`End::accounted`), and an issue with no claim, no worker start and no
/// account yet, are left untouched.
pub fn leave(repo: &Repo, gh: &GitHub, issue: u64, end: End, now: u64) -> Result<()> {
    if !end.accounted() {
        return Ok(());
    }
    let comments = gh.comments(issue)?;
    for comment in &comments {
        if afkconvoy_core::record::read_marker(account::ACCOUNT, &comment.body).is_some()
            && account::read(&comment.body, issue).is_err()
        {
            bail!(
                "issue {issue} has an unreadable work account; convert or repair it before writing another"
            );
        }
    }
    let name = claim_ref(repo.layout(), &gh.tracker, issue);
    let held = repo.remote().records(&[&name], Fresh::Process)?;
    let claimed = !held.is_empty();
    let held_since = held
        .first()
        .and_then(|record| Claim::read(issue, &record.subject, record.sha.clone()).since);
    let recorded = repo.remote().records(&[&repo.layout().every(GATE_KIND)], Fresh::Process)?;
    let gates: Vec<&str> = recorded.iter().map(|record| record.subject.as_str()).collect();
    let heard: Vec<Heard<'_>> = comments
        .iter()
        .map(|comment| Heard { at: comment.at, body: comment.body.as_str() })
        .collect();
    let (chip, model, cpus, bytes) = machine_of_host();
    let host = crate::this_host();
    let own = gh.repository()?;
    let tracker = match &gh.tracker {
        afkconvoy_core::tracker::Tracker::Own => own.as_str(),
        afkconvoy_core::tracker::Tracker::Other { tracker, .. } => tracker.as_str(),
    };
    let config = crate::config::load(&crate::config::file_of(repo))?;
    let mut lanes = vec![(config.gate_command.as_str(), "whole")];
    if let Some(gates) = &config.gates {
        lanes.push((gates.fast.command.as_str(), "fast"));
        lanes.push((gates.slow.command.as_str(), "slow"));
    }
    let Some(text) = account::comment(&account::Told {
        issue,
        repo: &own,
        tracker,
        version: crate::VERSION,
        outcome: end,
        lanes: &lanes,
        heard: &heard,
        gates: &gates,
        claimed,
        held_since,
        end: now,
        machine: Machine { chip: chip.as_deref(), model: model.as_deref(), cpus, bytes },
        host: &host,
    }) else {
        return Ok(());
    };
    let mut row = account::read(&text, issue).map_err(anyhow::Error::msg)?;
    let prior = comments.iter().find_map(|comment| account::read(&comment.body, issue).ok());
    let worktree = crate::orca::Source::of(repo)
        .ok()
        .and_then(|source| crate::orca::Worktree::of_issue(&source, issue).ok().flatten());
    for index in 0..row.workers.len() {
        let before = row.workers.get(index + 1).and_then(|worker| worker.start);
        let worker = &mut row.workers[index];
        let old = prior
            .as_ref()
            .and_then(|prior| {
                prior.workers.iter().find(|old| {
                    old.claim == worker.claim
                        && old.start == worker.start
                        && old.harness == worker.harness
                })
            })
            .cloned();
        let verdict_at = verdict_at(worker, &comments, before);
        let harness = worker.harness.as_deref().and_then(afkconvoy_core::harness::Harness::read);
        worker.goal = worktree
            .as_ref()
            .zip(harness)
            .zip(worker.start)
            .and_then(|((worktree, harness), start)| {
                crate::goal::account(&worktree.path, harness, start, before, verdict_at)
            })
            .or_else(|| old.as_ref().and_then(|old| old.goal.clone()));
        let old = old.and_then(|old| old.tokens);
        worker.tokens = worktree
            .as_ref()
            .and_then(|worktree| {
                let harness = afkconvoy_core::harness::Harness::read(worker.harness.as_deref()?)?;
                crate::session::tokens(&worktree.path, harness, worker.start?, before)
                    .ok()
                    .flatten()
            })
            .or(old);
    }
    let text = account::render(&row);
    let found = comments.iter().find(|comment| {
        afkconvoy_core::record::read_marker(account::ACCOUNT, &comment.body).is_some()
    });
    match found {
        Some(comment) if comment.body.trim() == text.trim() => Ok(()),
        Some(comment) => gh.edit_comment(comment.id, &text),
        None => gh.comment(issue, &text),
    }
}

/// When the verdict of `worker` stood: the stage record its row ends at,
/// for a worker with a stage; else the last verdict comment declared within
/// its row, before the next worker's start `before`. `None`: it declared
/// none.
fn verdict_at(
    worker: &account::Worker,
    comments: &[crate::gh::Comment],
    before: Option<u64>,
) -> Option<u64> {
    if worker.verdict.is_some() {
        return worker.end;
    }
    let start = worker.start?;
    let within: Vec<&crate::gh::Comment> = comments
        .iter()
        .filter(|comment| {
            comment.at.is_some_and(|at| at >= start && before.is_none_or(|before| at < before))
        })
        .collect();
    let verdict = afkconvoy_core::verdict::latest_verdict(
        within.iter().map(|comment| comment.body.as_str()),
    )?;
    within.get(verdict.comment)?.at
}

/// Refresh an existing account after its worker has stopped, while the
/// worktree and its session records still exist. Keep the original end:
/// finishing the worker must not lengthen the issue's wall clock.
pub(crate) fn refresh_stopped(repo: &Repo, gh: &GitHub, issue: u64) -> Result<()> {
    let comments = gh.comments(issue)?;
    let Some(row) = comments.iter().find_map(|comment| account::read(&comment.body, issue).ok())
    else {
        return Ok(());
    };
    let Some(at) = row.end else { return Ok(()) };
    let end = match row.outcome.as_deref() {
        Some("landed") => End::Landed,
        Some("closed") => End::Closed,
        Some("escalated") => End::Escalated,
        Some("ended") => End::Ended,
        Some("human-closed") => End::HumanClosed,
        _ => return Ok(()),
    };
    leave(repo, gh, issue, end, at)
}

/// Chip name, else model identifier; logical CPUs; memory in bytes. A fact
/// the host does not report is absent, and the account says so.
fn machine_of_host() -> (Option<String>, Option<String>, Option<u64>, Option<u64>) {
    if cfg!(target_os = "macos") {
        let chip = sysctl_text("machdep.cpu.brand_string");
        let model = sysctl_text("hw.model");
        let cpus = sysctl_text("hw.logicalcpu").and_then(|text| text.parse().ok());
        return (chip, model, cpus, crate::hot::memory());
    }
    let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let chip = info.lines().find_map(|line| {
        let rest = line.strip_prefix("model name")?;
        let name = rest.trim().trim_start_matches(':').trim();
        if name.is_empty() { None } else { Some(name.to_string()) }
    });
    let model = std::fs::read_to_string("/sys/devices/virtual/dmi/id/product_name")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let cpus = info.lines().filter(|line| line.starts_with("processor")).count();
    let cpus = if cpus == 0 { None } else { Some(cpus as u64) };
    (chip, model, cpus, crate::hot::memory())
}

fn sysctl_text(name: &str) -> Option<String> {
    let out = crate::process::output(Command::new("sysctl").args(["-n", name])).ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

//! A task's immutable destination, held on the remote before its first claim.
//! Parentless records cannot fast-forward one another: competing selections
//! converge on one branch, and every later stage reads that same decision.
use afkconvoy_core::config::Config;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    issue: String,
    branch: String,
}

fn refname(repo: &Repo, gh: &GitHub, issue: u64) -> String {
    repo.layout().under(&format!("target/{}", gh.tracker.claim_key(issue)))
}

fn existing(repo: &Repo, gh: &GitHub, issue: u64) -> Result<Option<Target>> {
    let name = refname(repo, gh, issue);
    let record = repo.remote_records(&name)?.pop();
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), &gh.tracker, issue);
    if let Some(claim) = repo.remote_records(&claim_ref)?.pop()
        && let Some(expected) = afkconvoy_core::record::read("convoy:claim", &claim.subject)
            .and_then(|fields| fields.get("target").map(str::to_string))
    {
        ensure!(
            record.as_ref().is_some_and(|r| r.sha == expected),
            "missing or contradictory pinned target for {}; restore {name} to {expected} before continuing",
            gh.tracker.issue(issue)
        );
    }
    let Some(record) = record else { return Ok(None) };
    let target: Target = serde_json::from_str(&record.subject).with_context(|| {
        format!(
            "unreadable target for {} at {name}; restore its pinned record",
            gh.tracker.issue(issue)
        )
    })?;
    ensure!(
        target.issue.eq_ignore_ascii_case(&gh.tracker.issue(issue)),
        "contradictory target at {name}; restore its pinned record"
    );
    validate(repo, &target.branch)?;
    Ok(Some(target))
}

/// Claims retain the identity of the target record, so losing or replacing
/// that ref never makes an explicit task look like a default-target task.
pub fn stamp_claim(
    repo: &Repo,
    tracker: &afkconvoy_core::tracker::Tracker,
    claim_ref: &str,
    subject: String,
) -> Result<String> {
    let Some(key) = repo.layout().kept(claim_ref).and_then(|name| name.strip_prefix("claim/"))
    else {
        return Ok(subject);
    };
    let afkconvoy_core::tracker::Claimed::Issue(issue) = tracker.claimed(key) else {
        return Ok(subject);
    };
    let gh = GitHub::of(repo)?;
    let name = refname(repo, &gh, issue);
    let sha = repo
        .remote_sha(&name)?
        .context("target disappeared before claim; restore its pinned record")?;
    Ok(format!("{subject}{}", afkconvoy_core::record::write("", &[("target", &sha)])))
}

fn validate(repo: &Repo, branch: &str) -> Result<()> {
    ensure!(
        repo.try_git(&["check-ref-format", &format!("refs/heads/{branch}")])?.ok,
        "invalid target branch {branch:?}; select an existing same-repo branch with afkconvoy target --issue <n> --branch <branch>"
    );
    ensure!(
        repo.remote_sha(&format!("refs/heads/{branch}"))?.is_some(),
        "target branch {branch:?} is missing; restore that branch on origin before continuing this task"
    );
    Ok(())
}

/// Reads the pinned branch; only a task with no record uses the configured base.
pub fn branch(repo: &Repo, config: &Config, issue: u64) -> Result<String> {
    let gh = GitHub::of(repo)?;
    Ok(existing(repo, &gh, issue)?.map_or_else(|| config.base_branch.clone(), |t| t.branch))
}

/// Selects exactly once. The default is pinned by the claim path too, so a
/// later config change cannot move a worker that has already started.
pub fn pin(repo: &Repo, config: &Config, issue: u64, selected: Option<&str>) -> Result<String> {
    let gh = GitHub::of(repo)?;
    if selected.is_some() {
        ensure!(
            !afkconvoy_core::tracker::is_public(&gh.visibility()?),
            "target selection requires a private tracker; configure one with afkconvoy run --tracker <owner/private-repo>"
        );
        let read = gh.issue(issue)?;
        ensure!(!read.pull_request && !read.is_closed(), "select a target for an open task issue");
        gh.tracker.to_take(&read.labels).map_err(anyhow::Error::msg)?;
    }
    if let Some(target) = existing(repo, &gh, issue)? {
        ensure!(
            selected.is_none_or(|b| b == target.branch),
            "{} is pinned to {:?}; changing a resolved target is refused; open a new task for another destination",
            gh.tracker.issue(issue),
            target.branch
        );
        return Ok(target.branch);
    }
    let branch = selected.unwrap_or(&config.base_branch);
    validate(repo, branch)?;
    if selected.is_some() {
        ensure!(
            crate::claims::Scan::read(repo)?.claim_of(issue).is_none(),
            "{} is already claimed; its target cannot change",
            gh.tracker.issue(issue)
        );
        // A candidate can survive its issue's claim. Never retarget it.
        ensure!(
            !crate::candidates::has_task(repo, issue)?,
            "{} still has candidates; its target cannot change",
            gh.tracker.issue(issue)
        );
    }
    let target = Target { issue: gh.tracker.issue(issue), branch: branch.to_string() };
    let sha = repo.record_commit(&serde_json::to_string(&target)?)?;
    let name = refname(repo, &gh, issue);
    let pushed = repo.try_git(&["push", "--quiet", REMOTE, &format!("{sha}:{name}")])?;
    if !pushed.ok {
        if let Some(won) = existing(repo, &gh, issue)? {
            ensure!(
                selected.is_none_or(|b| b == won.branch),
                "another selection pinned {} to {:?}; target change refused",
                target.issue,
                won.branch
            );
            return Ok(won.branch);
        }
        bail!(
            "could not pin target {}: {}; run this command again",
            target.issue,
            pushed.stderr.trim()
        );
    }
    Ok(target.branch)
}

pub fn select(repo: &Repo, issue: u64, selected: Option<&str>) -> Result<Value> {
    let config = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    let branch = match selected {
        Some(branch) => pin(repo, &config, issue, Some(branch))?,
        None => branch(repo, &config, issue)?,
    };
    Ok(json!({"issue": issue, "target_branch": branch}))
}

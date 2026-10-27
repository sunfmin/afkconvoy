//! Immutable task destinations. A delivery reference resolves its policy from
//! the delivery's one record, never from a separately editable branch copy.
use afkconvoy_core::config::Config;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::{REMOTE, RemoteRecord, Repo};
use afkconvoy_core::tracker::Tracker;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Target {
    issue: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<String>,
}

impl Target {
    fn resolve(&self, repo: &Repo) -> Result<String> {
        match (&self.branch, &self.delivery) {
            (Some(branch), None) => {
                ensure!(!crate::delivery::final_pr(branch, ""),
                    "reserved delivery branch requires its delivery reference; attach with afkconvoy delivery --jira <url> --final-target <branch> --issue <n>");
                validate(repo, branch)?;
                Ok(branch.clone())
            }
            (None, Some(id)) => crate::delivery::branch(repo, id),
            _ => bail!("contradictory task target; restore exactly one branch or delivery reference"),
        }
    }
}

fn refname(repo: &Repo, tracker: &Tracker, issue: u64) -> String {
    repo.layout().under(&format!("target/{}", tracker.claim_key(issue)))
}

fn existing(repo: &Repo, gh: &GitHub, issue: u64) -> Result<Option<Target>> {
    let name = refname(repo, &gh.tracker, issue);
    let record = repo.remote_records(&name)?.pop();
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), &gh.tracker, issue);
    let claim = repo.remote_records(&claim_ref)?.pop();
    let target = decode(&gh.tracker.issue(issue), &name, record.as_ref(), claim.as_ref())?;
    if let Some(target) = &target { target.resolve(repo)?; }
    Ok(target)
}

fn decode(issue: &str, name: &str, record: Option<&RemoteRecord>, claim: Option<&RemoteRecord>) -> Result<Option<Target>> {
    if let Some(claim) = claim
        && let Some(expected) = afkconvoy_core::record::read("convoy:claim", &claim.subject)
            .and_then(|fields| fields.get("target").map(str::to_string))
    {
        ensure!(record.as_ref().is_some_and(|r| r.sha == expected),
            "missing or contradictory pinned target for {}; restore {name} to {expected} before continuing", issue);
    }
    let Some(record) = record else { return Ok(None) };
    let target: Target = serde_json::from_str(&record.subject).with_context(|| {
        format!("unreadable target for {} at {name}; restore its pinned record", issue)
    })?;
    ensure!(target.issue.eq_ignore_ascii_case(issue), "contradictory target at {name}; restore its pinned record");
    ensure!(target.branch.is_some() != target.delivery.is_some(),
        "contradictory target at {name}; restore exactly one branch or delivery reference");
    Ok(Some(target))
}

/// Resolve delivery references from the rebuild's same remote snapshot.
pub fn snapshot_branch(repo: &Repo, config: &Config, tracker: &Tracker, issue: u64, records: &[RemoteRecord]) -> Result<String> {
    let name = refname(repo, tracker, issue);
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), tracker, issue);
    let target = decode(&tracker.issue(issue), &name,
        records.iter().find(|r| r.name == name), records.iter().find(|r| r.name == claim_ref))?;
    match target {
        None => Ok(config.base_branch.clone()),
        Some(Target { delivery: Some(id), .. }) => crate::delivery::snapshot_branch(repo, &id, records),
        Some(Target { branch: Some(branch), .. }) => {
            ensure!(!crate::delivery::final_pr(&branch, ""), "delivery branch requires its immutable delivery reference; restore the task target");
            Ok(branch)
        }
        _ => bail!("contradictory task target; restore its record"),
    }
}

/// Claims retain the target record's identity so losing it never means default.
pub fn stamp_claim(repo: &Repo, tracker: &Tracker, claim_ref: &str, subject: String) -> Result<String> {
    let Some(key) = repo.layout().kept(claim_ref).and_then(|name| name.strip_prefix("claim/")) else { return Ok(subject) };
    let afkconvoy_core::tracker::Claimed::Issue(issue) = tracker.claimed(key) else { return Ok(subject) };
    let gh = GitHub::of(repo)?;
    let name = refname(repo, &gh.tracker, issue);
    let sha = repo.remote_sha(&name)?.context("target disappeared before claim; restore its pinned record")?;
    Ok(format!("{subject}{}", afkconvoy_core::record::write("", &[("target", &sha)])))
}

fn validate(repo: &Repo, branch: &str) -> Result<()> {
    ensure!(repo.try_git(&["check-ref-format", &format!("refs/heads/{branch}")])?.ok,
        "invalid target branch {branch:?}; select an existing same-repo branch with afkconvoy target --issue <n> --branch <branch>");
    ensure!(repo.remote_sha(&format!("refs/heads/{branch}"))?.is_some(),
        "the remote has no branch {branch:?}: the task's target branch is missing; restore that branch on origin before continuing this task");
    Ok(())
}

pub fn branch(repo: &Repo, config: &Config, issue: u64) -> Result<String> {
    match existing(repo, &GitHub::of(repo)?, issue)? {
        Some(target) => target.resolve(repo),
        None => Ok(config.base_branch.clone()),
    }
}

fn selectable(gh: &GitHub, issue: u64) -> Result<()> {
    ensure!(!afkconvoy_core::tracker::is_public(&gh.visibility()?),
        "target selection requires a private tracker; configure one with afkconvoy run --tracker <owner/private-repo>");
    let read = gh.issue(issue)?;
    ensure!(!read.pull_request && !read.is_closed(), "select a target for an open task issue");
    gh.tracker.to_take(&read.labels).map_err(anyhow::Error::msg)?;
    Ok(())
}

fn write(repo: &Repo, gh: &GitHub, issue: u64, target: Target) -> Result<String> {
    let branch = target.resolve(repo)?;
    if let Some(won) = existing(repo, gh, issue)? {
        ensure!(target == won, "{} is pinned to another target; changing a resolved target is refused; open a new task", target.issue);
        return won.resolve(repo);
    }
    ensure!(crate::claims::Scan::read(repo)?.claim_of(issue).is_none(),
        "{} is already claimed; its target cannot change", target.issue);
    ensure!(!crate::candidates::has_task(repo, issue)?,
        "{} still has candidates; its target cannot change", target.issue);
    let sha = repo.record_commit(&serde_json::to_string(&target)?)?;
    let name = refname(repo, &gh.tracker, issue);
    let pushed = repo.try_git(&["push", "--quiet", REMOTE, &format!("{sha}:{name}")])?;
    // Always discover even after success, covering simultaneous selection.
    if let Some(won) = existing(repo, gh, issue)? {
        ensure!(target == won, "another selection pinned {} to a different target; target change refused", target.issue);
        return Ok(branch);
    }
    bail!("could not pin target {}: {}; run this command again", target.issue, pushed.stderr.trim())
}

/// A first claim pins the default too; a later config change cannot move it.
pub fn pin(repo: &Repo, config: &Config, issue: u64, selected: Option<&str>) -> Result<String> {
    let gh = GitHub::of(repo)?;
    if selected.is_some() { selectable(&gh, issue)?; }
    if let Some(target) = existing(repo, &gh, issue)? {
        let branch = target.resolve(repo)?;
        ensure!(selected.is_none_or(|b| b == branch),
            "{} is pinned to {:?}; changing a resolved target is refused; open a new task for another destination", gh.tracker.issue(issue), branch);
        return Ok(branch);
    }
    write(repo, &gh, issue, Target { issue: gh.tracker.issue(issue), branch: Some(selected.unwrap_or(&config.base_branch).to_string()), delivery: None })
}

pub fn attach(repo: &Repo, issue: u64, id: &str) -> Result<String> {
    let gh = GitHub::of(repo)?;
    selectable(&gh, issue)?;
    write(repo, &gh, issue, Target { issue: gh.tracker.issue(issue), branch: None, delivery: Some(id.to_string()) })
}

pub fn delivery(repo: &Repo, issue: u64) -> Result<Option<String>> {
    Ok(existing(repo, &GitHub::of(repo)?, issue)?.and_then(|t| t.delivery))
}

pub fn attached(repo: &Repo, id: &str) -> Result<Vec<String>> {
    let mut issues = vec![];
    for record in repo.remote_records(&repo.layout().every("target/"))? {
        let target: Target = serde_json::from_str(&record.subject).with_context(|| format!("unreadable task target at {}; restore it before inspecting deliveries", record.name))?;
        if target.delivery.as_deref() == Some(id) { issues.push(target.issue); }
    }
    issues.sort();
    Ok(issues)
}

pub fn select(repo: &Repo, issue: u64, selected: Option<&str>) -> Result<Value> {
    let config = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    let branch = match selected {
        Some(branch) => pin(repo, &config, issue, Some(branch))?,
        None => branch(repo, &config, issue)?,
    };
    let mut answer = json!({"issue": issue, "target_branch": branch});
    if let Some(id) = delivery(repo, issue)? { answer["delivery"] = json!(id); }
    Ok(answer)
}

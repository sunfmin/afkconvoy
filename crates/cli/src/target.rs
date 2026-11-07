//! Immutable task destinations. A delivery reference resolves its policy from
//! the delivery's one record, never from a separately editable branch copy.
use afkconvoy_core::config::Config;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::Repo;
use crate::remote::{Fresh, Listing, Push, RemoteRecord};
use afkconvoy_core::tracker::Tracker;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Target {
    issue: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<crate::delivery::Reference>,
}

impl Target {
    fn resolve(&self, repo: &Repo) -> Result<String> {
        self.resolve_known(repo, &[], &[])
    }

    /// `resolve`, except a branch ref this look already asked for is not read
    /// again. `asked` is those ref names; `listed` is what the remote had.
    fn resolve_known(
        &self,
        repo: &Repo,
        asked: &[&str],
        listed: &[RemoteRecord],
    ) -> Result<String> {
        match (&self.branch, &self.delivery) {
            (Some(branch), None) => {
                ensure!(
                    !crate::delivery::final_pr(branch, ""),
                    "reserved delivery branch requires its delivery reference; attach with afkconvoy delivery --jira <url> --final-target <branch> --issue <n>"
                );
                validate_known(repo, branch, asked, listed)?;
                Ok(branch.clone())
            }
            (None, Some(reference)) => crate::delivery::resolve(repo, reference, None),
            _ => {
                bail!("contradictory task target; restore exactly one branch or delivery reference")
            }
        }
    }
}

fn refname(repo: &Repo, tracker: &Tracker, issue: u64) -> String {
    repo.layout().under(&format!("target/{}", tracker.claim_key(issue)))
}

fn existing(repo: &Repo, gh: &GitHub, issue: u64) -> Result<Option<Target>> {
    let name = refname(repo, &gh.tracker, issue);
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), &gh.tracker, issue);
    let listed = repo.remote().refs(&[&name, &claim_ref], Fresh::Process)?;
    let target = from_listing(repo, &gh.tracker, issue, &listed)?;
    if let Some(target) = &target {
        target.resolve(repo)?;
    }
    Ok(target)
}

/// The target record in `listed`, if this issue has one. A missing target and
/// a missing claim are the absence of a pin: no subject is read.
fn from_listing(
    repo: &Repo,
    tracker: &Tracker,
    issue: u64,
    listed: &[RemoteRecord],
) -> Result<Option<Target>> {
    let name = refname(repo, tracker, issue);
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), tracker, issue);
    let mut records: Vec<RemoteRecord> = listed
        .iter()
        .filter(|record| record.name == name || record.name == claim_ref)
        .cloned()
        .collect();
    if records.is_empty() {
        return Ok(None);
    }
    repo.read_record_subjects(&mut records)?;
    decode(
        &tracker.issue(issue),
        &name,
        records.iter().find(|record| record.name == name),
        records.iter().find(|record| record.name == claim_ref),
    )
}

/// Where issue `issue` lands, as one read of the remote saw it.
pub struct Seen {
    pub branch: String,
    /// Tip of `branch` when this look's listing held `tip_ref`.
    pub tip: Option<String>,
    pub tip_ref: String,
    /// `tip_ref` was among the refs this look asked the remote for, so a
    /// missing tip is the remote having no such branch.
    pub asked: bool,
}

/// The branch issue `issue` lands on, and that branch's tip when the one
/// listing already held it. The target ref, the claim ref and the configured
/// base are read live together, once. Ownership is checked before resolving
/// the target or writing a turn receipt.
pub fn once(repo: &Repo, config: &Config, tracker: &Tracker, issue: u64) -> Result<Seen> {
    let name = refname(repo, tracker, issue);
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), tracker, issue);
    let base_ref = format!("refs/heads/{}", config.base_branch);
    let asked_refs = [name.as_str(), claim_ref.as_str(), base_ref.as_str()];
    let listed = repo.remote().refs(&asked_refs, Fresh::Live)?;
    crate::claims::on_this_host(repo, issue, &crate::host_settings::host_name(), Fresh::Process)?;
    let target = from_listing(repo, tracker, issue, &listed)?;
    let branch = match target {
        None => config.base_branch.clone(),
        Some(target) => target.resolve_known(repo, &asked_refs, &listed)?,
    };
    let tip_ref = format!("refs/heads/{branch}");
    let tip = listed.iter().find(|record| record.name == tip_ref).map(|record| record.sha.clone());
    let asked = asked_refs.contains(&tip_ref.as_str());
    Ok(Seen { branch, tip, tip_ref, asked })
}

fn decode(
    issue: &str,
    name: &str,
    record: Option<&RemoteRecord>,
    claim: Option<&RemoteRecord>,
) -> Result<Option<Target>> {
    if let Some(claim) = claim
        && let Some(expected) = afkconvoy_core::record::read("convoy:claim", &claim.subject)
            .and_then(|fields| fields.get("target").map(str::to_string))
    {
        ensure!(
            record.as_ref().is_some_and(|r| r.sha == expected),
            "missing or contradictory pinned target for {}; restore {name} to {expected} before continuing",
            issue
        );
    }
    let Some(record) = record else { return Ok(None) };
    let target: Target = serde_json::from_str(&record.subject).with_context(|| {
        format!("unreadable target for {} at {name}; restore its pinned record", issue)
    })?;
    ensure!(
        target.issue.eq_ignore_ascii_case(issue),
        "contradictory target at {name}; restore its pinned record"
    );
    ensure!(
        target.branch.is_some() != target.delivery.is_some(),
        "contradictory target at {name}; restore exactly one branch or delivery reference"
    );
    Ok(Some(target))
}

/// Resolve delivery references from the rebuild's same remote snapshot.
pub fn snapshot_branch(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    issue: u64,
    records: &[RemoteRecord],
) -> Result<String> {
    let name = refname(repo, tracker, issue);
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), tracker, issue);
    let target = decode(
        &tracker.issue(issue),
        &name,
        records.iter().find(|r| r.name == name),
        records.iter().find(|r| r.name == claim_ref),
    )?;
    match target {
        None => Ok(config.base_branch.clone()),
        Some(Target { delivery: Some(reference), .. }) => {
            crate::delivery::resolve(repo, &reference, Some(records))
        }
        Some(Target { branch: Some(branch), .. }) => {
            ensure!(
                !crate::delivery::final_pr(&branch, ""),
                "delivery branch requires its immutable delivery reference; restore the task target"
            );
            Ok(branch)
        }
        _ => bail!("contradictory task target; restore its record"),
    }
}

/// Claims retain the target record's identity so losing it never means default.
pub fn stamp_claim(
    repo: &Repo,
    tracker: &Tracker,
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
    let name = refname(repo, &gh.tracker, issue);
    let sha = repo
        .remote()
        .sha(&name, Fresh::Process)?
        .context("target disappeared before claim; restore its pinned record")?;
    Ok(format!("{subject}{}", afkconvoy_core::record::write("", &[("target", &sha)])))
}

/// A branch ref named in `asked` is taken from `listed` and not read again.
/// With nothing asked, the remote is read for the branch.
fn validate_known(
    repo: &Repo,
    branch: &str,
    asked: &[&str],
    listed: &[RemoteRecord],
) -> Result<()> {
    let name = format!("refs/heads/{branch}");
    ensure!(
        repo.ref_name_ok(&name)?,
        "invalid target branch {branch:?}; select an existing same-repo branch with afkconvoy target --issue <n> --branch <branch>"
    );
    let present = if asked.contains(&name.as_str()) {
        listed.iter().any(|record| record.name == name)
    } else {
        repo.remote().sha(&name, Fresh::Process)?.is_some()
    };
    ensure!(
        present,
        "the remote has no branch {branch:?}: the task's target branch is missing; restore that branch on origin before continuing this task"
    );
    Ok(())
}

pub fn branch(repo: &Repo, config: &Config, issue: u64) -> Result<String> {
    match existing(repo, &GitHub::of(repo)?, issue)? {
        Some(target) => target.resolve(repo),
        None => Ok(config.base_branch.clone()),
    }
}

fn selectable(gh: &GitHub, issue: u64) -> Result<()> {
    ensure!(
        !afkconvoy_core::tracker::is_public(&gh.visibility()?),
        "target selection requires a private tracker; configure one with afkconvoy run --tracker <owner/private-repo>"
    );
    let read = gh.issue(issue)?;
    ensure!(!read.pull_request && !read.is_closed(), "select a target for an open task issue");
    gh.tracker.to_take(&read.labels).map_err(anyhow::Error::msg)?;
    Ok(())
}

fn write(repo: &Repo, gh: &GitHub, issue: u64, target: Target, selecting: bool) -> Result<String> {
    let branch = target.resolve(repo)?;
    if let Some(won) = existing(repo, gh, issue)? {
        ensure!(
            target == won,
            "{} is pinned to another target: {}; changing a resolved target is refused; inspect afkconvoy recovery --issue {issue} and open a new task",
            target.issue,
            serde_json::to_string(&won)?
        );
        return won.resolve(repo);
    }
    if selecting {
        ensure!(
            crate::claims::Scan::read(repo)?.claim_of(issue).is_none(),
            "{} is already claimed; its target cannot change",
            target.issue
        );
        ensure!(
            !crate::candidates::has_task(repo, issue)?,
            "{} still has candidates; its target cannot change",
            target.issue
        );
    }
    let sha = repo.record_commit(&serde_json::to_string(&target)?)?;
    let name = refname(repo, &gh.tracker, issue);
    let pushed = repo.remote().try_push_retried(&Push::set(&sha, &name), None, || Ok(false))?;
    // Always discover even after success, covering simultaneous selection.
    if let Some(won) = existing(repo, gh, issue)? {
        ensure!(
            target == won,
            "another selection pinned {} to a different target; target change refused",
            target.issue
        );
        return Ok(branch);
    }
    bail!("could not pin target {}: {}; run this command again", target.issue, pushed.stderr.trim())
}

/// A first claim pins the default too; a later config change cannot move it.
pub fn pin(repo: &Repo, config: &Config, issue: u64, selected: Option<&str>) -> Result<String> {
    let gh = GitHub::of(repo)?;
    if selected.is_some() {
        selectable(&gh, issue)?;
    }
    if let Some(target) = existing(repo, &gh, issue)? {
        let branch = target.resolve(repo)?;
        ensure!(
            selected.is_none_or(|b| b == branch),
            "{} is pinned to {:?}; changing a resolved target is refused; open a new task for another destination",
            gh.tracker.issue(issue),
            branch
        );
        return Ok(branch);
    }
    write(
        repo,
        &gh,
        issue,
        Target {
            issue: gh.tracker.issue(issue),
            branch: Some(selected.unwrap_or(&config.base_branch).to_string()),
            delivery: None,
        },
        selected.is_some(),
    )
}

pub fn attach(repo: &Repo, issue: u64, id: &str) -> Result<String> {
    let gh = GitHub::of(repo)?;
    selectable(&gh, issue)?;
    if existing(repo, &gh, issue)?.is_none() {
        let prs = gh.open_prs()?;
        let prs: Vec<_> =
            prs.iter().filter(|pr| pr.closes.contains(&issue)).map(|pr| pr.number).collect();
        let comments = gh.comments(issue)?;
        let recorded =
            afkconvoy_core::worker::recorded_branches(comments.iter().map(|c| c.body.as_str()));
        let heads = repo.remote().heads(Fresh::Process)?;
        let branches = afkconvoy_core::recovery::own_branches(&heads, &recorded, None);
        let worktree = crate::orca::Worktree::of_issue(&crate::orca::Source::of(repo)?, issue)?;
        ensure!(
            prs.is_empty() && branches.is_empty() && worktree.is_none(),
            "{} has unaccounted work: PRs {prs:?}, branches {branches:?}, worktree {:?}; preserve and account for this evidence before attachment; inspect afkconvoy recovery --issue {issue}",
            gh.tracker.issue(issue),
            worktree.map(|w| w.path)
        );
    }
    write(
        repo,
        &gh,
        issue,
        Target {
            issue: gh.tracker.issue(issue),
            branch: None,
            delivery: Some(crate::delivery::reference(repo, id)?),
        },
        true,
    )
}

pub fn delivery(repo: &Repo, issue: u64) -> Result<Option<String>> {
    Ok(existing(repo, &GitHub::of(repo)?, issue)?.and_then(|t| t.delivery).map(|r| r.identity))
}

pub fn attached(repo: &Repo, id: &str) -> Result<Vec<String>> {
    let mut issues = vec![];
    for record in repo.remote().records(&[&repo.layout().every("target/")], Fresh::Process)? {
        let target: Target = serde_json::from_str(&record.subject).with_context(|| {
            format!(
                "unreadable task target at {}; restore it before inspecting deliveries",
                record.name
            )
        })?;
        if target.delivery.as_ref().is_some_and(|r| r.identity == id) {
            issues.push(target.issue);
        }
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
    if let Some(id) = delivery(repo, issue)? {
        answer["delivery"] = json!(id);
    }
    Ok(answer)
}

/// Read-only destinations, including unbound tasks and per-task errors, for
/// plan and doctor. Membership stays solely in the remote target record.
pub fn listing(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    open: &[afkconvoy_core::plan::Issue],
    listing: &Listing,
) -> Result<Value> {
    let mut rows = vec![];
    for issue in open.iter().filter(|issue| tracker.holds(&issue.labels)) {
        let name = refname(repo, tracker, issue.number);
        let record = listing.records.iter().find(|r| r.name == name);
        let mut answer = json!({"issue": issue.number, "tracker_issue": tracker.issue(issue.number),
            "pinned": record.is_some(), "delivery": null, "configured_base": config.base_branch});
        if let Some(record) = record
            && let Ok(target) = serde_json::from_str::<Target>(&record.subject)
        {
            answer["delivery"] = json!(target.delivery.map(|r| r.identity));
        }
        match snapshot_branch(repo, config, tracker, issue.number, &listing.records) {
            Ok(branch) => {
                answer["target_branch"] = json!(branch);
                let head = format!("refs/heads/{branch}");
                if !listing.heads.iter().any(|(name, _)| name == &head) {
                    answer["error"] =
                        json!(format!("target branch {branch:?} is missing; restore it on origin"));
                }
            }
            Err(error) => answer["error"] = json!(format!("{error:#}")),
        }
        rows.push(answer);
    }
    Ok(json!(rows))
}

/// What `afkconvoy target --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Run before dispatch or intake: `afkconvoy target --issue 7 --branch delivery/ABC-123`. Then give the issue its ready or flow label as usual. The worker cuts from that branch, opens its internal PR against it, gates and lands there, and closes only its task issue. The repo's configured base and its release destination stay as configured.

## Who runs it

The operator, in a checkout of the repo, before making the task dispatchable.

## What it reads and writes

Without --branch, reads the target, or the configured base for a task not yet resolved. Selection requires an existing branch on origin and an open issue in this checkout's private tracker scope. It writes an immutable remote record, keyed by the tracker issue identity. A first claim also pins the configured base when no selection exists. Stages, continuation and landing retain it across restarts. Reading a malformed record or a missing branch refuses; it never falls back to the configured base.

## What it answers

{"issue", "target_branch"}, with "delivery" for an attached task, and "woke" after selection. Selection is idempotent for the same branch. Errors answer {"error": …} with a non-zero exit.

## What trips you up

A different resolved target, a claimed task, remaining candidates, an unreadable tracker, a foreign repo label or a missing branch is refused with a diagnostic. A resolved target cannot be changed: open a new task for another destination.

Jira delivery branches are reserved: attach tasks through `afkconvoy delivery --jira <url> --final-target <branch> --issue <n>`. Their target records pin a reference to the immutable delivery definition; they never copy its branch settings. A missing or replaced definition blocks work instead of selecting the configured base."#;

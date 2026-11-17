//! Immutable task destinations (docs/adr/0006). A claim carries its task's
//! pin in its own record, so a takeover keeps it and it ends with the claim
//! (sunfmin/control#1327). Before a claim, an operator's branch selection is
//! the issue's `convoy:target` marker comment, and a delivery's tasks are its
//! one member record. Nothing of a target is kept per issue on the remote; the
//! separate `target/<n>` records an older afkconvoy wrote are read once,
//! carried into these homes and removed (`carry_legacy`).
use afkconvoy_core::claims::{Claim, Pin};
use afkconvoy_core::config::Config;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::delivery::Reference;
use crate::gh::{Comment, GitHub};
use crate::git::Repo;
use crate::remote::{Fresh, Listing, Push, RemoteRecord};
use afkconvoy_core::tracker::Tracker;

/// The marker of an operator's branch selection in an issue's comments.
const SELECTED: &str = "convoy:target";

/// Where a task lands.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Branch(String),
    Delivery(Reference),
}

impl Target {
    fn of(pin: &Pin) -> Option<Target> {
        match pin {
            Pin::Branch(branch) => Some(Target::Branch(branch.clone())),
            Pin::Delivery { identity, record } => Some(Target::Delivery(Reference {
                identity: identity.clone(),
                record: record.clone(),
            })),
            Pin::Legacy(_) => None,
        }
    }

    fn pin(&self) -> Pin {
        match self {
            Target::Branch(branch) => Pin::Branch(branch.clone()),
            Target::Delivery(reference) => Pin::Delivery {
                identity: reference.identity.clone(),
                record: reference.record.clone(),
            },
        }
    }

    fn describe(&self) -> String {
        match self {
            Target::Branch(branch) => format!("branch {branch:?}"),
            Target::Delivery(reference) => format!("delivery {}", reference.identity),
        }
    }

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
        match self {
            Target::Branch(branch) => {
                ensure!(
                    !crate::delivery::final_pr(branch, ""),
                    "reserved delivery branch requires its delivery reference; attach with afkconvoy delivery --jira <url> --final-target <branch> --issue <n>"
                );
                validate_known(repo, branch, asked, listed)?;
                Ok(branch.clone())
            }
            Target::Delivery(reference) => crate::delivery::resolve(repo, reference, None),
        }
    }

    /// The branch, from one snapshot of the remote.
    fn resolve_snapshot(&self, repo: &Repo, records: &[RemoteRecord]) -> Result<String> {
        match self {
            Target::Delivery(reference) => crate::delivery::resolve(repo, reference, Some(records)),
            Target::Branch(branch) => {
                ensure!(
                    !crate::delivery::final_pr(branch, ""),
                    "delivery branch requires its immutable delivery reference; restore the task target"
                );
                Ok(branch.clone())
            }
        }
    }
}

/// A separate target record as an older afkconvoy wrote one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    issue: String,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    delivery: Option<Reference>,
}

fn legacy_ref(repo: &Repo, tracker: &Tracker, issue: u64) -> String {
    repo.layout().under(&format!("target/{}", tracker.claim_key(issue)))
}

fn read_legacy(issue: &str, record: &RemoteRecord) -> Result<Target> {
    let name = &record.name;
    let legacy: Legacy = serde_json::from_str(&record.subject).with_context(|| {
        format!("unreadable target for {issue} at {name}; restore its pinned record")
    })?;
    ensure!(
        legacy.issue.eq_ignore_ascii_case(issue),
        "contradictory target at {name}; restore its pinned record"
    );
    match (legacy.branch, legacy.delivery) {
        (Some(branch), None) => Ok(Target::Branch(branch)),
        (None, Some(reference)) => Ok(Target::Delivery(reference)),
        _ => bail!(
            "contradictory target at {name}; restore exactly one branch or delivery reference"
        ),
    }
}

/// The record of a delivery's tasks: one per delivery, never per task.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Members {
    /// The delivery record every task was attached at.
    record: String,
    /// The tasks, each as its tracker names it, sorted.
    tasks: Vec<String>,
}

const MEMBERS_KIND: &str = "delivery-tasks/";

fn members_ref(repo: &Repo, id: &str) -> String {
    repo.layout().under(&format!("{MEMBERS_KIND}{id}"))
}

fn read_members(record: &RemoteRecord) -> Result<Members> {
    serde_json::from_str(&record.subject).with_context(|| {
        format!("unreadable delivery tasks at {}; restore the delivery's task record", record.name)
    })
}

/// The delivery `issue` was attached to, among the member records `listed`.
fn member_of(repo: &Repo, issue: &str, listed: &[RemoteRecord]) -> Result<Option<Target>> {
    let prefix = repo.layout().under(MEMBERS_KIND);
    let mut found = None;
    for record in listed.iter().filter(|record| record.name.starts_with(&prefix)) {
        let members = read_members(record)?;
        if !members.tasks.iter().any(|task| task.eq_ignore_ascii_case(issue)) {
            continue;
        }
        let identity = record.name[prefix.len()..].to_string();
        ensure!(
            found.is_none(),
            "{issue} is attached to two deliveries; restore exactly one delivery reference"
        );
        found = Some(Target::Delivery(Reference { identity, record: members.record }));
    }
    Ok(found)
}

/// The branch an operator selected in `comments`: the first marker wins, so
/// a selection is never changed once made.
fn selected_in(comments: &[Comment]) -> Option<Target> {
    comments.iter().find_map(|comment| {
        afkconvoy_core::record::markers(SELECTED, &comment.body)
            .into_iter()
            .find_map(|fields| fields.get("branch").map(|branch| Target::Branch(branch.into())))
    })
}

/// Where issue `issue` lands, from what one look read: the pin its claim
/// carries; else a separate record not yet carried; else its selection —
/// a branch in its comments or a delivery's tasks. `None`: the configured
/// base. `comments` is read only when nothing before it decides.
fn decide(
    repo: &Repo,
    tracker: &Tracker,
    issue: u64,
    listed: &[RemoteRecord],
    comments: impl FnOnce() -> Result<Vec<Comment>>,
) -> Result<Option<Target>> {
    let named = tracker.issue(issue);
    let claim_ref = afkconvoy_core::claims::claim_ref(repo.layout(), tracker, issue);
    let pin = listed
        .iter()
        .find(|record| record.name == claim_ref)
        .and_then(|record| Claim::read(issue, &record.subject, record.sha.clone()).pin);
    let legacy_name = legacy_ref(repo, tracker, issue);
    let legacy = listed.iter().find(|record| record.name == legacy_name);
    match pin {
        Some(Pin::Legacy(expected)) => {
            ensure!(
                legacy.is_some_and(|record| record.sha == expected),
                "missing or contradictory pinned target for {named}; restore {legacy_name} to {expected} before continuing"
            );
            return read_legacy(&named, legacy.expect("checked above")).map(Some);
        }
        Some(pin) => return Ok(Target::of(&pin)),
        None => {}
    }
    if let Some(record) = legacy {
        return read_legacy(&named, record).map(Some);
    }
    let delivery = member_of(repo, &named, listed)?;
    let branch = selected_in(&comments()?);
    match (branch, delivery) {
        (Some(_), Some(_)) => bail!(
            "contradictory task target for {named}: a selected branch and a delivery; restore exactly one"
        ),
        (branch, delivery) => Ok(branch.or(delivery)),
    }
}

/// The refs one look at issue `issue`'s target reads, besides the comments.
fn patterns(repo: &Repo, tracker: &Tracker, issue: u64) -> [String; 3] {
    [
        afkconvoy_core::claims::claim_ref(repo.layout(), tracker, issue),
        legacy_ref(repo, tracker, issue),
        repo.layout().every(MEMBERS_KIND),
    ]
}

fn existing(repo: &Repo, gh: &GitHub, issue: u64) -> Result<Option<Target>> {
    let names = patterns(repo, &gh.tracker, issue);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let listed = repo.remote().records(&names, Fresh::Process)?;
    let target = decide(repo, &gh.tracker, issue, &listed, || gh.comments(issue))?;
    if let Some(target) = &target {
        target.resolve(repo)?;
    }
    Ok(target)
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
/// listing already held it. The claim ref and the configured base are read
/// live together, once. Ownership is checked before resolving the target or
/// writing a turn receipt.
/// An unclaimed issue's comments are read without a quota probe.
pub fn once(repo: &Repo, config: &Config, gh: &GitHub, issue: u64) -> Result<Seen> {
    let tracker = &gh.tracker;
    let base_ref = format!("refs/heads/{}", config.base_branch);
    let [claim_ref, legacy, members] = patterns(repo, tracker, issue);
    let asked_refs = [claim_ref.as_str(), legacy.as_str(), members.as_str(), base_ref.as_str()];
    let mut listed = repo.remote().refs(&asked_refs, Fresh::Live)?;
    repo.read_record_subjects(&mut listed)?;
    crate::claims::on_this_host(repo, issue, &crate::host_settings::host_name(), Fresh::Process)?;
    let target = decide(repo, tracker, issue, &listed, || {
        gh.note_without_probe()?;
        gh.comments(issue)
    })?;
    let branch = match target {
        None => config.base_branch.clone(),
        Some(target) => target.resolve_known(repo, &asked_refs, &listed)?,
    };
    let tip_ref = format!("refs/heads/{branch}");
    let tip = listed.iter().find(|record| record.name == tip_ref).map(|record| record.sha.clone());
    let asked = asked_refs.contains(&tip_ref.as_str());
    Ok(Seen { branch, tip, tip_ref, asked })
}

/// The branch issue `issue` lands on, from the rebuild's same remote
/// snapshot; `comments` are its comments when the caller read them, else
/// they are read only if nothing in the snapshot decides.
pub fn snapshot_branch(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    issue: u64,
    records: &[RemoteRecord],
    comments: Option<&[Comment]>,
) -> Result<String> {
    let read = || match comments {
        Some(comments) => Ok(comments.to_vec()),
        None => GitHub::of(repo)?.comments(issue),
    };
    match decide(repo, tracker, issue, records, read)? {
        None => Ok(config.base_branch.clone()),
        Some(target) => target.resolve_snapshot(repo, records),
    }
}

/// The pin a claim on issue `issue` carries: the one `taken` carries, or
/// what its legacy pin reads as; for a new claim, the issue's selection, or
/// the configured base. A later config change cannot move it.
pub fn for_claim(repo: &Repo, config: &Config, issue: u64, taken: Option<&Claim>) -> Result<Pin> {
    if let Some(target) = taken.and_then(|claim| claim.pin.as_ref()).and_then(Target::of) {
        return Ok(target.pin());
    }
    let gh = GitHub::of(repo)?;
    Ok(match existing(repo, &gh, issue)? {
        Some(target) => target.pin(),
        None => Pin::Branch(config.base_branch.clone()),
    })
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

/// Refuses a selection of `target` where the issue already has another, or
/// where it is claimed or still has candidates → whether it already has
/// `target`.
fn may_select(repo: &Repo, gh: &GitHub, issue: u64, target: &Target) -> Result<bool> {
    let named = gh.tracker.issue(issue);
    if let Some(won) = existing(repo, gh, issue)? {
        ensure!(
            *target == won,
            "{named} is pinned to another target: {}; changing a resolved target is refused; inspect afkconvoy recovery --issue {issue} and open a new task",
            won.describe()
        );
        return Ok(true);
    }
    ensure!(
        crate::claims::Scan::read(repo)?.claim_of(issue).is_none(),
        "{named} is already claimed; its target cannot change"
    );
    ensure!(
        !crate::candidates::has_task(repo, issue)?,
        "{named} still has candidates; its target cannot change"
    );
    Ok(false)
}

/// After writing a selection, reads it back: a simultaneous selection may
/// have won.
fn confirm(repo: &Repo, gh: &GitHub, issue: u64, target: &Target) -> Result<String> {
    let won = existing(repo, gh, issue)?;
    ensure!(
        won.as_ref() == Some(target),
        "another selection pinned {} to a different target; target change refused",
        gh.tracker.issue(issue)
    );
    target.resolve(repo)
}

/// Selects `branch` for issue `issue`, as its `convoy:target` comment.
pub fn choose(repo: &Repo, issue: u64, branch: &str) -> Result<String> {
    let gh = GitHub::of(repo)?;
    selectable(&gh, issue)?;
    let target = Target::Branch(branch.to_string());
    if may_select(repo, &gh, issue, &target)? {
        return target.resolve(repo);
    }
    target.resolve(repo)?;
    gh.comment(issue, &selection_comment(branch))?;
    confirm(repo, &gh, issue, &target)
}

fn selection_comment(branch: &str) -> String {
    format!(
        "{}\n**afkconvoy** — this task lands on `{branch}`. The selection cannot change: open a new task for another destination.",
        afkconvoy_core::record::marker(SELECTED, &[("branch", branch)])
    )
}

/// Adds `task` to the tasks of the delivery `reference` names, with one
/// leased push of its member record.
fn join(repo: &Repo, reference: &Reference, task: &str) -> Result<()> {
    let name = members_ref(repo, &reference.identity);
    for _ in 0..5 {
        let now = repo.remote().records(&[&name], Fresh::Live)?.pop();
        let mut members = match &now {
            Some(record) => read_members(record)?,
            None => Members { record: reference.record.clone(), tasks: vec![] },
        };
        ensure!(
            members.record == reference.record,
            "delivery {} has tasks attached at another record; restore {name}",
            reference.identity
        );
        if members.tasks.iter().any(|known| known.eq_ignore_ascii_case(task)) {
            return Ok(());
        }
        members.tasks.push(task.to_string());
        members.tasks.sort();
        let sha = repo.record_commit(&serde_json::to_string(&members)?)?;
        let lease = now.as_ref().map_or("", |record| record.sha.as_str());
        let push = Push::set(&sha, &name).leased(lease);
        if repo.remote().try_push_retried(&push, None, || Ok(true))?.ok {
            return Ok(());
        }
    }
    bail!("could not attach {task} to delivery {}; run this command again", reference.identity)
}

pub fn attach(repo: &Repo, issue: u64, id: &str) -> Result<String> {
    let gh = GitHub::of(repo)?;
    selectable(&gh, issue)?;
    let target = Target::Delivery(crate::delivery::reference(repo, id)?);
    if may_select(repo, &gh, issue, &target)? {
        return target.resolve(repo);
    }
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
    let Target::Delivery(reference) = &target else { unreachable!("built as a delivery") };
    join(repo, reference, &gh.tracker.issue(issue))?;
    confirm(repo, &gh, issue, &target)
}

pub fn delivery(repo: &Repo, issue: u64) -> Result<Option<String>> {
    Ok(match existing(repo, &GitHub::of(repo)?, issue)? {
        Some(Target::Delivery(reference)) => Some(reference.identity),
        _ => None,
    })
}

/// The tasks attached to delivery `id`, open or closed, as their tracker
/// names them.
pub fn attached(repo: &Repo, id: &str) -> Result<Vec<String>> {
    let name = members_ref(repo, id);
    let legacy = repo.layout().every("target/");
    let mut issues = vec![];
    for record in repo.remote().records(&[&name, &legacy], Fresh::Process)? {
        if record.name == name {
            issues.extend(read_members(&record)?.tasks);
            continue;
        }
        // Not yet carried into the delivery's tasks (`carry_legacy`).
        let legacy: Legacy = serde_json::from_str(&record.subject).with_context(|| {
            format!(
                "unreadable task target at {}; restore it before inspecting deliveries",
                record.name
            )
        })?;
        if legacy.delivery.as_ref().is_some_and(|r| r.identity == id) {
            issues.push(legacy.issue);
        }
    }
    issues.sort();
    issues.dedup();
    Ok(issues)
}

pub fn select(repo: &Repo, issue: u64, selected: Option<&str>) -> Result<Value> {
    let config = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    let branch = match selected {
        Some(branch) => choose(repo, issue, branch)?,
        None => branch(repo, &config, issue)?,
    };
    let mut answer = json!({"issue": issue, "target_branch": branch});
    if let Some(id) = delivery(repo, issue)? {
        answer["delivery"] = json!(id);
    }
    Ok(answer)
}

/// Read-only destinations, including unbound tasks and per-task errors, for
/// plan and doctor, from one listing of the remote and one read of the open
/// issues with their comments.
pub fn listing(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    open: &crate::gh::OpenIssues,
    listing: &Listing,
) -> Result<Value> {
    let mut rows = vec![];
    for issue in open.rows.iter().filter(|issue| tracker.holds(&issue.labels)) {
        let comments = open.comments(issue.number);
        let decided =
            decide(repo, tracker, issue.number, &listing.records, || Ok(comments.to_vec()));
        let mut answer = json!({"issue": issue.number, "tracker_issue": tracker.issue(issue.number),
            "pinned": matches!(decided, Ok(Some(_))), "delivery": null,
            "configured_base": config.base_branch});
        if let Ok(Some(Target::Delivery(reference))) = &decided {
            answer["delivery"] = json!(reference.identity);
        }
        let branch = decided.and_then(|target| match target {
            None => Ok(config.base_branch.clone()),
            Some(target) => target.resolve_snapshot(repo, &listing.records),
        });
        match branch {
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

/// The separate target records an older afkconvoy left, as the fleet's
/// listing of the remote has them.
pub fn legacy_left(fleet: &crate::fleet::Fleet) -> Vec<RemoteRecord> {
    fleet.records_at_open(&fleet.repo.layout().every("target/"))
}

/// Reads each separate target record an older afkconvoy left once, carries
/// what an open task still needs into where a target lives now, and removes
/// it (sunfmin/control#1327): a claim's legacy pin becomes its own pin; a
/// selected branch other than the configured base becomes the issue's
/// `convoy:target` comment; a delivery's task joins the delivery's tasks,
/// closed or open, so its handoff still counts it. Those of closed issues
/// keep nothing else. A record not carried whole is kept for the next tick,
/// and so is one whose claim is another instance's: its holder carries it,
/// or, an older afkconvoy, still reads it; a dead one's is carried once a
/// takeover pins its claim. → the records removed.
pub fn carry_legacy(
    fleet: &mut crate::fleet::Fleet,
    me: &afkconvoy_core::claims::InstanceId,
) -> Result<Vec<String>> {
    let records = legacy_left(fleet);
    let (mut removed, mut failed) = (vec![], vec![]);
    for record in records {
        match carry_one(fleet, me, &record) {
            Ok(true) => removed.push(record.name),
            Ok(false) => {}
            Err(error) => failed.push(format!("{}: {error:#}", record.name)),
        }
    }
    ensure!(failed.is_empty(), "could not carry {}", failed.join("; "));
    Ok(removed)
}

fn carry_one(
    fleet: &mut crate::fleet::Fleet,
    me: &afkconvoy_core::claims::InstanceId,
    record: &RemoteRecord,
) -> Result<bool> {
    let repo = &fleet.repo;
    let tracker = fleet.gh.tracker.clone();
    let key = repo.layout().kept(&record.name).and_then(|name| name.strip_prefix("target/"));
    let issue = match key.map(|key| tracker.claimed(key)) {
        Some(afkconvoy_core::tracker::Claimed::Issue(issue)) => issue,
        // Another tracker's record is that fleet's to carry.
        _ => return Ok(false),
    };
    let target = read_legacy(&tracker.issue(issue), record)?;
    if let Some(claim) = fleet.scan.claim_of(issue).cloned()
        && matches!(claim.pin, None | Some(Pin::Legacy(_)))
    {
        if !claim.is_held_by(me) {
            return Ok(false);
        }
        if !crate::claims::repin(repo, &mut fleet.scan, &claim, &target.pin())? {
            return Ok(false);
        }
    }
    match &target {
        Target::Delivery(reference) => join(repo, reference, &tracker.issue(issue))?,
        Target::Branch(branch) if *branch != fleet.config.base_branch => {
            if !fleet.gh.issue(issue)?.is_closed()
                && selected_in(&fleet.gh.comments(issue)?).is_none()
            {
                fleet.gh.comment(issue, &selection_comment(branch))?;
            }
        }
        Target::Branch(_) => {}
    }
    let push = Push::delete(&record.name).leased(&record.sha);
    Ok(repo.remote().try_push_retried(&push, None, || Ok(true))?.ok)
}

/// What `afkconvoy target --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Run before dispatch or intake: `afkconvoy target --issue 7 --branch delivery/ABC-123`. Then give the issue its ready or flow label as usual. The worker cuts from that branch, opens its internal PR against it, gates and lands there, and closes only its task issue. The repo's configured base and its release destination stay as configured.

## Who runs it

The operator, in a checkout of the repo, before making the task dispatchable.

## What it reads and writes

Without --branch, reads the target, or the configured base for a task with none. Selection requires an existing branch on origin and an open, unclaimed issue in this checkout's private tracker scope.

Where the target lives: a selection is the issue's `<!--convoy:target branch=…-->` comment; the first one stands. A task attached to a Jira delivery is named in that delivery's one task record (`afkconvoy delivery --help`). Each claim carries its task's target in its own record: a claim pins the selection, or the configured base when there is none, and a later config change cannot move it. A takeover by another host keeps the claim's target, and it ends with the claim — nothing of it is left per issue on the remote. A selection outlives a released claim, so the next claim pins it again. Reading a malformed record or a missing branch refuses; it never falls back to the configured base.

An older afkconvoy kept a separate `target/<n>` record per issue. The acting tick reads each once, carries what an open task needs into a claim, a selection comment or a delivery's tasks, and removes it; `doctor` reports any still left.

## What it answers

{"issue", "target_branch"}, with "delivery" for an attached task, and "woke" after selection. Selection is idempotent for the same branch. Errors answer {"error": …} with a non-zero exit.

## What trips you up

A different resolved target, a claimed task, remaining candidates, an unreadable tracker, a foreign repo label or a missing branch is refused with a diagnostic. A resolved target cannot be changed: open a new task for another destination.

Jira delivery branches are reserved: attach tasks through `afkconvoy delivery --jira <url> --final-target <branch> --issue <n>`. Their targets pin a reference to the immutable delivery definition; they never copy its branch settings. A missing or replaced definition blocks work instead of selecting the configured base."#;

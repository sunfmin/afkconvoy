//! Jira references group task contributions; immutable remote records own
//! branch policy, while GitHub owns the human-reviewed final PR. A handoff
//! certifies the aggregate against the final target and leaves the merge to
//! a human.
use std::time::Duration;

use afkconvoy_core::candidate::{Chain, Phase};
use afkconvoy_core::gate::{GateCommand, Tree};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::remote::{Fresh, Push, RemoteRecord};

const BRANCH_PREFIX: &str = "delivery/jira-";
const MARKER: &str = "<!--convoy:delivery ";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Delivery {
    pub identity: String,
    pub jira: String,
    pub repository: String,
    pub branch: String,
    pub final_target: String,
    pub initial: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    delivery: String,
    pr: u64,
}

/// A task pins the definition's object identity, not copies of its policy.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reference {
    pub identity: String,
    pub record: String,
}

pub fn reference(repo: &Repo, id: &str) -> Result<Reference> {
    let name = refname(repo, id)?;
    let rows = repo.remote().records(&[&name], Fresh::Process)?;
    let delivery = decode(repo, id, rows.first())?;
    ensure!(
        delivery.repository.eq_ignore_ascii_case(&GitHub::of(repo)?.repository()?),
        "delivery {id} belongs to code repo {}; select a delivery in this code repo with afkconvoy delivery --jira <url>",
        delivery.repository
    );
    delivery.validate(repo)?;
    Ok(Reference { identity: id.to_string(), record: rows[0].sha.clone() })
}

pub fn resolve(
    repo: &Repo,
    reference: &Reference,
    snapshot: Option<&[RemoteRecord]>,
) -> Result<String> {
    let name = refname(repo, &reference.identity)?;
    let read;
    let records = match snapshot {
        Some(records) => records,
        None => {
            read = repo.remote().records(&[&name], Fresh::Process)?;
            &read
        }
    };
    let record = records.iter().find(|r| r.name == name);
    ensure!(
        record.is_some_and(|r| r.sha == reference.record),
        "missing or contradictory delivery {}; restore {name} to {} before continuing attached tasks",
        reference.identity,
        reference.record
    );
    let delivery = decode(repo, &reference.identity, record)?;
    if snapshot.is_none() {
        delivery.validate(repo)?;
    }
    Ok(delivery.branch)
}

/// The reserved head namespace also protects a final PR whose body was edited.
pub fn final_pr(branch: &str, body: &str) -> bool {
    branch.starts_with(BRANCH_PREFIX) || body.contains(MARKER)
}

fn jira_reference(input: &str) -> Result<(String, String)> {
    let invalid = "use a full Jira reference: https://<site>[/context]/browse/ABC-123 (no query, fragment or credentials)";
    let rest = input.trim().strip_prefix("https://").context(invalid)?;
    let (host, path) = rest.split_once('/').context(invalid)?;
    let (hostname, port) = match host.rsplit_once(':') {
        Some((hostname, port)) => {
            let port: u16 = port.parse().context(invalid)?;
            ensure!(port != 0, "{invalid}");
            (hostname, (port != 443).then_some(port))
        }
        None => (host, None),
    };
    ensure!(
        !hostname.is_empty()
            && hostname.split('.').all(|label| !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
            && !path.contains(['?', '#', '%', '\\'])
            && !path.split('/').any(|p| p == "." || p == ".." || p.is_empty()),
        "{invalid}"
    );
    let (context, key) = format!("/{path}")
        .rsplit_once("/browse/")
        .map(|(a, b)| (a.to_string(), b.to_ascii_uppercase()))
        .context(invalid)?;
    let (project, number) = key.rsplit_once('-').context(invalid)?;
    ensure!(
        project.starts_with(|c: char| c.is_ascii_alphabetic())
            && project.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            && !number.is_empty()
            && number.bytes().all(|c| c.is_ascii_digit())
            && context.bytes().all(|c| c.is_ascii_alphanumeric() || b"/_.-".contains(&c)),
        "{invalid}"
    );
    let authority = match port {
        Some(port) => format!("{}:{port}", hostname.to_ascii_lowercase()),
        None => hostname.to_ascii_lowercase(),
    };
    Ok((format!("https://{authority}{context}/browse/{key}"), key))
}

fn identity(jira: &str, repository: &str) -> String {
    format!("{:016x}", afkconvoy_core::gate::key(&format!("{repository}\n{jira}")))
}

fn refname(repo: &Repo, id: &str) -> Result<String> {
    ensure!(
        id.len() == 16 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid delivery identity {id:?}; inspect afkconvoy delivery --help and restore the task's delivery reference"
    );
    Ok(repo.layout().under(&format!("delivery/{id}")))
}

fn decode(repo: &Repo, id: &str, record: Option<&RemoteRecord>) -> Result<Delivery> {
    let name = refname(repo, id)?;
    let record = record.with_context(|| format!("missing delivery {id} at {name}; restore its immutable record before continuing attached tasks"))?;
    let delivery: Delivery = serde_json::from_str(&record.subject)
        .with_context(|| format!("unreadable delivery at {name}; restore its immutable record"))?;
    let (jira, key) = jira_reference(&delivery.jira)?;
    ensure!(
        delivery.identity == id
            && jira == delivery.jira
            && identity(&jira, &delivery.repository) == id
            && delivery.branch == format!("{BRANCH_PREFIX}{key}-{id}")
            && !final_pr(&delivery.final_target, ""),
        "contradictory delivery at {name}; restore its immutable record"
    );
    ensure!(
        repo.ref_name_ok(&format!("refs/heads/{}", delivery.final_target))?,
        "invalid final target in delivery {id}; restore its immutable record"
    );
    Ok(delivery)
}

pub(crate) fn load(repo: &Repo, id: &str) -> Result<Delivery> {
    let name = refname(repo, id)?;
    let rows = repo.remote().records(&[&name], Fresh::Process)?;
    decode(repo, id, rows.first())
}

impl Delivery {
    fn validate(&self, repo: &Repo) -> Result<()> {
        let tip = repo.remote().fetched(&format!("refs/heads/{}", self.branch), Fresh::Process)?.with_context(|| {
            format!(
                "delivery {} branch {:?} is missing; restore the owned branch before continuing",
                self.identity, self.branch
            )
        })?;
        ensure!(
            repo.within(&self.initial, &tip)?,
            "delivery {} branch history contradicts its initial commit; restore the owned branch before continuing",
            self.identity
        );
        ensure!(
            repo.remote()
                .sha(&format!("refs/heads/{}", self.final_target), Fresh::Process)?
                .is_some(),
            "delivery {} final target {:?} is missing; restore it before continuing",
            self.identity,
            self.final_target
        );
        Ok(())
    }

    fn marker(&self) -> String {
        format!("{MARKER}id={}-->", self.identity)
    }
}

fn create(
    repo: &Repo,
    gh: &GitHub,
    jira: &str,
    key: &str,
    repository: &str,
    final_target: &str,
) -> Result<Delivery> {
    let id = identity(jira, repository);
    let name = refname(repo, &id)?;
    let existing = repo.remote().records(&[&name], Fresh::Process)?;
    if !existing.is_empty() {
        let found = decode(repo, &id, existing.first())?;
        ensure!(
            found.jira == jira
                && found.repository == repository
                && found.final_target == final_target,
            "delivery {id} already owns final target {:?}; conflicting request refused; inspect with afkconvoy delivery --jira {jira}",
            found.final_target
        );
        found.validate(repo)?;
        return Ok(found);
    }
    ensure!(
        !afkconvoy_core::tracker::is_public(&gh.visibility()?),
        "delivery creation requires a private tracker; configure afkconvoy run --tracker <owner/private-repo>"
    );
    ensure!(
        repo.ref_name_ok(&format!("refs/heads/{final_target}"))?
            && !final_pr(final_target, "")
            && !final_target.starts_with("convoy-"),
        "invalid final target {final_target:?}; name an existing code branch on origin"
    );
    let initial = repo.remote().fetched(&format!("refs/heads/{final_target}"), Fresh::Process)?
        .with_context(|| format!("final target {final_target:?} does not exist on origin; create it before creating a delivery"))?;
    let delivery = Delivery {
        identity: id.clone(),
        jira: jira.to_string(),
        repository: repository.to_string(),
        branch: format!("{BRANCH_PREFIX}{key}-{id}"),
        final_target: final_target.to_string(),
        initial,
    };
    let branch_ref = format!("refs/heads/{}", delivery.branch);
    // Both absent expectations are checked in one receive-pack transaction.
    // Even a coincidentally identical existing branch is not ours to adopt.
    let sha = repo.record_commit(&serde_json::to_string(&delivery)?)?;
    let zero = "0".repeat(sha.len());
    let target_ref = format!("refs/heads/{final_target}");
    let pushed = repo.remote().publish(&[
        crate::remote::Update { name: &name, old: &zero, new: &sha },
        crate::remote::Update { name: &branch_ref, old: &zero, new: &delivery.initial },
        crate::remote::Update { name: &target_ref, old: &delivery.initial, new: &delivery.initial },
    ]);
    // Lost receipts and competing creators both discover the actual remote.
    let records = repo.remote().records(&[&name], Fresh::Process)?;
    if !records.is_empty() {
        let found = decode(repo, &id, records.first())?;
        ensure!(
            found.jira == jira
                && found.repository == repository
                && found.final_target == final_target,
            "concurrent request selected another delivery definition; inspect afkconvoy delivery --jira {jira}"
        );
        found.validate(repo)?;
        return Ok(found);
    }
    bail!(
        "delivery creation refused; an unrelated branch {:?}, moved final target or rejected atomic push may exist: {pushed:?}; inspect origin and retry without overwriting its work",
        delivery.branch
    )
}

fn receipt(repo: &Repo, delivery: &Delivery) -> Result<Option<Receipt>> {
    let name = repo.layout().under(&format!("delivery-pr/{}", delivery.identity));
    let rows = repo.remote().records(&[&name], Fresh::Process)?;
    rows.first()
        .map(|r| {
            let read: Receipt = serde_json::from_str(&r.subject).with_context(|| {
                format!("unreadable final PR receipt at {name}; restore it before continuing")
            })?;
            ensure!(
                read.delivery == delivery.identity,
                "contradictory final PR receipt at {name}; restore it before continuing"
            );
            Ok(read)
        })
        .transpose()
}

fn keep_receipt(repo: &Repo, delivery: &Delivery, pr: u64) -> Result<()> {
    if let Some(found) = receipt(repo, delivery)? {
        ensure!(
            found.pr == pr,
            "delivery has conflicting final PRs; inspect GitHub and restore its final PR receipt"
        );
        return Ok(());
    }
    let name = repo.layout().under(&format!("delivery-pr/{}", delivery.identity));
    let sha = repo.record_commit(&serde_json::to_string(&Receipt {
        delivery: delivery.identity.clone(),
        pr,
    })?)?;
    let pushed = repo.remote().try_push_retried(&Push::set(&sha, &name), None, || Ok(false))?;
    let found = receipt(repo, delivery)?.context("final PR exists but its receipt was not recorded; rerun afkconvoy delivery --jira <url> --final-target <branch>")?;
    ensure!(
        found.pr == pr,
        "conflicting final PR receipt: {}; inspect the delivery before retrying",
        pushed.stderr.trim()
    );
    Ok(())
}

fn discover(
    repo: &Repo,
    gh: &GitHub,
    delivery: &Delivery,
) -> Result<Option<crate::gh::DeliveryPr>> {
    let saved = receipt(repo, delivery)?;
    let prs = gh.delivery_prs()?;
    let matching: Vec<_> = prs
        .into_iter()
        .filter(|pr| {
            !pr.fork
                && (pr.head_branch == delivery.branch
                    || pr.body.contains(&delivery.marker())
                    || saved.as_ref().is_some_and(|r| r.pr == pr.number))
        })
        .collect();
    ensure!(
        matching.len() <= 1,
        "ambiguous final PRs for delivery {}; inspect GitHub and its receipt; no PR will be created",
        delivery.identity
    );
    let found = matching.into_iter().next();
    if let Some(pr) = &found {
        ensure!(
            pr.head_branch == delivery.branch
                && pr.base_branch == delivery.final_target
                && pr.body.contains(&delivery.marker())
                && saved.as_ref().is_none_or(|r| r.pr == pr.number),
            "final PR #{} conflicts with delivery {}; restore its head, base and delivery marker before continuing",
            pr.number,
            delivery.identity
        );
    } else {
        ensure!(
            saved.is_none(),
            "recorded final PR is missing for delivery {}; restore its GitHub metadata before continuing",
            delivery.identity
        );
    }
    Ok(found)
}

fn has_diff(repo: &Repo, delivery: &Delivery) -> Result<bool> {
    let base = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.final_target), Fresh::Process)?
        .context("final target disappeared; restore it before retrying")?;
    let head = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.branch), Fresh::Process)?
        .context("delivery branch disappeared; restore it before retrying")?;
    let diff = repo.try_git(&["diff", "--quiet", &format!("{base}...{head}"), "--"])?;
    if diff.ok {
        return Ok(false);
    }
    // Distinguish a meaningful diff (exit 1) from a failed comparison.
    repo.git(&["diff", "--stat", &format!("{base}...{head}"), "--"])?;
    Ok(true)
}

fn review(repo: &Repo, gh: &GitHub, delivery: &Delivery, write: bool) -> Result<Value> {
    delivery.validate(repo)?;
    if let Some(pr) = discover(repo, gh, delivery)? {
        if write {
            keep_receipt(repo, delivery, pr.number)?;
        }
        return Ok(
            json!({"number": pr.number, "url": pr.url, "state": pr.state, "draft": pr.draft, "merged": pr.merged}),
        );
    }
    if !has_diff(repo, delivery)? {
        return Ok(json!({"pending": "no meaningful diff from the final target"}));
    }
    if !write {
        return Ok(
            json!({"pending": "meaningful diff exists; the next acting tick or delivery creation retry will open the final Draft PR"}),
        );
    }
    let body = format!(
        "{}\n\nJira delivery: {}\n\nThis final review PR gathers internal task contributions. A human owns colleague review and merging this PR.\n",
        delivery.marker(),
        delivery.jira
    );
    let title = format!("Jira delivery: {}", delivery.jira);
    let created = gh.open_draft_pr(&delivery.final_target, &delivery.branch, &title, &body);
    // Never repeat an uncertain write before re-reading all actual PR states.
    let pr = discover(repo, gh, delivery)?.with_context(|| match created {
        Ok(_) => "GitHub did not expose the created final PR; retry this command to discover it".to_string(),
        Err(error) => format!("final PR creation pending: {error:#}; retry this command to discover it before another attempt"),
    })?;
    keep_receipt(repo, delivery, pr.number)?;
    Ok(
        json!({"number": pr.number, "url": pr.url, "state": pr.state, "draft": pr.draft, "merged": pr.merged}),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CheckedTask {
    issue: String,
    result: String,
    blocking: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Evidence {
    delivery: String,
    delivery_head: String,
    final_target_head: String,
    tasks: Vec<CheckedTask>,
    gate_tree: String,
    gate_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Settlement {
    delivery: String,
    pr: u64,
    observed: String,
    complete: bool,
    on_target: bool,
    delivery_head: String,
    final_target_head: String,
    tasks: Vec<CheckedTask>,
}

struct Snapshot {
    delivery_head: String,
    final_target_head: String,
    tasks: Vec<CheckedTask>,
}

fn evidence_ref(repo: &Repo, id: &str) -> String {
    repo.layout().under(&format!("delivery-evidence/{id}"))
}

fn settlement_ref(repo: &Repo, id: &str) -> String {
    repo.layout().under(&format!("delivery-settled/{id}"))
}

fn read_record<T: for<'de> Deserialize<'de>>(repo: &Repo, name: &str) -> Result<Option<T>> {
    let rows = repo.remote().records(&[name], Fresh::Process)?;
    let Some(row) = rows.first() else { return Ok(None) };
    let value = serde_json::from_str(&row.subject)
        .with_context(|| format!("unreadable record at {name}; restore it before continuing"))?;
    Ok(Some(value))
}

/// Publishes `body` at `name`. `replace` moves an existing ref onto the new
/// snapshot; a settlement (`replace` false) keeps the first record.
fn publish(repo: &Repo, name: &str, body: &str, replace: bool) -> Result<()> {
    let rows = repo.remote().records(&[name], Fresh::Process)?;
    if rows.first().is_some_and(|row| row.subject == body || !replace) {
        return Ok(());
    }
    let sha = repo.record_commit(body)?;
    let zero = "0".repeat(sha.len());
    let old = rows.first().map(|row| row.sha.clone()).unwrap_or(zero);
    let _pushed = repo.remote().publish(&[crate::remote::Update { name, old: &old, new: &sha }])?;
    let now = repo.remote().records(&[name], Fresh::Process)?;
    let got = now
        .first()
        .with_context(|| format!("record {name} was not published; rerun the handoff"))?;
    if got.subject != body && replace {
        bail!("record {name} changed while it was published; rerun the handoff");
    }
    Ok(())
}

fn result_of(blocking: &[String]) -> String {
    if blocking.is_empty() {
        "implemented".to_string()
    } else if blocking.iter().any(|reason| reason.contains("not planned")) {
        "not_planned".to_string()
    } else if blocking
        .iter()
        .any(|reason| reason.contains("closed as") || reason.contains("without a landing"))
    {
        "abandoned".to_string()
    } else {
        "incomplete".to_string()
    }
}

fn candidate_blocks(repo: &Repo, issue: u64) -> Result<Vec<String>> {
    let prefix = repo.layout().under("candidates/");
    let mut reasons = vec![];
    for record in repo.remote().records(&[&format!("{prefix}*")], Fresh::Process)? {
        let chain: Chain = serde_json::from_str(&record.subject).with_context(|| {
            format!(
                "unreadable candidate record {}; restore it before a delivery handoff",
                record.name
            )
        })?;
        let Some(last) = chain.candidates.iter().rfind(|c| c.issue == Some(issue)) else {
            continue;
        };
        let reason = match last.phase {
            Phase::Red => Some(format!("landing {} failed its gate", last.id)),
            Phase::Cancelled | Phase::Cancelling => {
                Some(format!("landing {} was cancelled", last.id))
            }
            Phase::Building | Phase::Validating | Phase::Green | Phase::Slow => {
                Some(format!("landing {} is still in progress", last.id))
            }
            Phase::Landed => None,
        };
        if let Some(reason) = reason {
            reasons.push(reason);
        }
    }
    Ok(reasons)
}

fn retained_work(repo: &Repo, issue: u64, tip: &str) -> Result<Option<String>> {
    let source = match crate::orca::Source::of(repo) {
        Ok(source) => source,
        Err(error) => return Ok(Some(format!("worktrees could not be read: {error:#}"))),
    };
    let Some(work) = crate::orca::Worktree::of_issue(&source, issue)? else {
        return Ok(None);
    };
    if !work.path.is_dir() {
        return Ok(None);
    }
    let checkout = Repo::new(work.path.clone());
    let dirty = crate::gate::off_commit(&checkout, None)?;
    let head = checkout.git(&["rev-parse", "HEAD"])?;
    let contained = repo.within(&head, tip)?;
    if dirty.is_empty() && contained {
        return Ok(None);
    }
    Ok(Some(format!(
        "#{issue} retains work at {} (dirty {}, head on the delivery branch: {contained})",
        work.path.display(),
        !dirty.is_empty()
    )))
}

fn assess(repo: &Repo, gh: &GitHub, delivery: &Delivery) -> Result<Snapshot> {
    let delivery_head = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.branch), Fresh::Process)?
        .with_context(|| {
            format!("delivery branch {:?} disappeared; restore it", delivery.branch)
        })?;
    let final_target_head = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.final_target), Fresh::Process)?
        .with_context(|| {
            format!("final target {:?} disappeared; restore it", delivery.final_target)
        })?;
    let names = crate::target::attached(repo, &delivery.identity)?;
    let joins = crate::landing::joins_in(repo, &gh.tracker, &[&delivery_head])?;
    let claims = crate::claims::Scan::read(repo)?;
    let open = gh.open_prs()?;
    let mut tasks = vec![];
    for name in names {
        let number = gh.tracker.issue_named(&name).with_context(|| {
            format!("unreadable attached task {name}; restore its target record")
        })?;
        let mut blocking = vec![];
        let read = gh.issue(number)?;
        let landed = joins.iter().any(|join| join.issue() == Some(number));
        match read.state_reason.as_deref() {
            Some("not_planned") => {
                blocking.push(format!("{name} was closed as not planned and is not implemented"));
            }
            Some(reason) if read.is_closed() && reason != "completed" => {
                blocking.push(format!("{name} was closed as {reason} and is not implemented"));
            }
            _ if read.is_closed() && !landed => {
                blocking.push(format!("{name} is closed without a landing on the delivery branch"));
            }
            _ if !read.is_closed() => blocking.push(format!("{name} is still open")),
            _ => {}
        }
        if claims.claim_of(number).is_some() {
            blocking.push(format!("{name} has an unresolved claim"));
        }
        let mut prs: Vec<u64> =
            open.iter().filter(|pr| pr.closes.contains(&number)).map(|pr| pr.number).collect();
        prs.sort_unstable();
        for pr in prs {
            blocking.push(format!("{name} has open internal PR #{pr}"));
        }
        let mut blockers = gh.open_blockers(number)?;
        blockers.sort_unstable();
        for blocker in blockers {
            blocking.push(format!("{name} is blocked by #{blocker}"));
        }
        blocking.extend(candidate_blocks(repo, number)?);
        if let Some(reason) = retained_work(repo, number, &delivery_head)? {
            blocking.push(reason);
        }
        blocking.sort();
        let result = result_of(&blocking);
        tasks.push(CheckedTask { issue: name, result, blocking });
    }
    Ok(Snapshot { delivery_head, final_target_head, tasks })
}

fn implemented(tasks: &[CheckedTask]) -> bool {
    !tasks.is_empty() && tasks.iter().all(|task| task.blocking.is_empty())
}

fn reasons_of(tasks: &[CheckedTask]) -> Vec<String> {
    let mut reasons: Vec<String> = tasks.iter().flat_map(|task| task.blocking.clone()).collect();
    if tasks.is_empty() {
        reasons.push("no attached tasks".to_string());
    }
    reasons
}

fn pr_terminal(pr: &crate::gh::DeliveryPr) -> Option<&'static str> {
    if pr.merged || pr.state.eq_ignore_ascii_case("merged") {
        Some("merged")
    } else if pr.state.eq_ignore_ascii_case("closed") {
        Some("closed")
    } else {
        None
    }
}

fn on_target(
    repo: &Repo,
    delivery: &Delivery,
    pr: &crate::gh::DeliveryPr,
    head: &str,
) -> Result<bool> {
    if pr.base_branch != delivery.final_target {
        return Ok(false);
    }
    let target = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.final_target), Fresh::Process)?
        .context("final target disappeared")?;
    repo.within(head, &target)
}

fn observe(
    repo: &Repo,
    delivery: &Delivery,
    pr: &crate::gh::DeliveryPr,
    snapshot: &Snapshot,
) -> Result<Option<Settlement>> {
    let Some(observed) = pr_terminal(pr) else { return Ok(None) };
    let name = settlement_ref(repo, &delivery.identity);
    if let Some(found) = read_record::<Settlement>(repo, &name)? {
        return Ok(Some(found));
    }
    let contained = on_target(repo, delivery, pr, &snapshot.delivery_head)?;
    let complete = observed == "merged" && contained && implemented(&snapshot.tasks);
    let settlement = Settlement {
        delivery: delivery.identity.clone(),
        pr: pr.number,
        observed: observed.to_string(),
        complete,
        on_target: contained,
        delivery_head: snapshot.delivery_head.clone(),
        final_target_head: snapshot.final_target_head.clone(),
        tasks: snapshot.tasks.clone(),
    };
    publish(repo, &name, &serde_json::to_string(&settlement)?, false)?;
    read_record(repo, &name)
}

fn settlement_differs(
    found: &Settlement,
    snapshot: &Snapshot,
    observed: Option<&str>,
    on: bool,
) -> bool {
    let complete = observed == Some("merged") && on && implemented(&snapshot.tasks);
    found.tasks != snapshot.tasks
        || found.delivery_head != snapshot.delivery_head
        || found.final_target_head != snapshot.final_target_head
        || found.observed != observed.unwrap_or("")
        || found.complete != complete
        || found.on_target != on
}

fn certifies(repo: &Repo, evidence: &Evidence, snapshot: &Snapshot, now: u64) -> Result<bool> {
    if evidence.delivery_head != snapshot.delivery_head
        || evidence.final_target_head != snapshot.final_target_head
        || evidence.tasks != snapshot.tasks
        || !implemented(&snapshot.tasks)
    {
        return Ok(false);
    }
    let config = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    let command = GateCommand::new(&config.gate_command);
    if evidence.gate_command != command.text() {
        return Ok(false);
    }
    let tree = Tree::named(&evidence.gate_tree);
    Ok(crate::gate::recorded(repo, &tree, &command, now)?.is_some())
}

enum Aggregate {
    Green { tree: String, command: String, source: &'static str },
    Red { detail: String },
    Conflict,
}

fn aggregate(repo: &Repo, delivery: &Delivery, host: &str, now: u64) -> Result<Aggregate> {
    let base = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.final_target), Fresh::Process)?
        .context("final target disappeared")?;
    let head = repo
        .remote()
        .fetched(&format!("refs/heads/{}", delivery.branch), Fresh::Process)?
        .context("delivery branch disappeared")?;
    let message = format!(
        "Jira delivery {} aggregate check\n\nThis commit is not published to {}.",
        delivery.identity, delivery.final_target
    );
    let Some(commit) = crate::turn::merged_onto(repo, &base, &head, &message, now)? else {
        return Ok(Aggregate::Conflict);
    };
    let path = std::env::temp_dir().join(format!("afkconvoy-delivery-{}", crate::unique()));
    repo.git(&["worktree", "add", "--quiet", "--detach", &path.to_string_lossy(), &commit])?;
    let checked = |checkout: &Repo| -> Result<Aggregate> {
        let config = crate::config::load(&checkout.dir.join(crate::config::FILE))?;
        let command = GateCommand::new(&config.gate_command);
        let gated = crate::gate::gated(
            checkout,
            &command,
            host,
            now,
            Duration::from_secs(config.gate_seconds()),
            None,
            None,
        )?;
        let tree = crate::gate::head_tree(checkout)?.name().to_string();
        match gated {
            crate::gate::Gated::Green { source, .. } => {
                Ok(Aggregate::Green { tree, command: command.text().to_string(), source })
            }
            crate::gate::Gated::Red { ran, excerpt, termination } => {
                let why = termination.map(|end| end.description()).unwrap_or_else(|| {
                    format!("the aggregate gate is red (exit {})", ran.exit_code)
                });
                let tail = excerpt.into_iter().rev().take(5).collect::<Vec<_>>().into_iter().rev();
                Ok(Aggregate::Red {
                    detail: format!("{why}\n{}", tail.collect::<Vec<_>>().join("\n")),
                })
            }
        }
    };
    let checkout = Repo::new(path.clone());
    let result = checked(&checkout);
    // A failed removal must not hide a red gate, and a green result must not
    // leave the temporary worktree behind.
    if let Err(error) = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()])
        && result.is_ok()
    {
        return Err(error);
    }
    result
}

fn pr_body(delivery: &Delivery, snapshot: &Snapshot, evidence: &Evidence) -> String {
    let mut tasks = String::new();
    for task in &snapshot.tasks {
        tasks.push_str(&format!("- {} {}\n", task.issue, task.result));
    }
    format!(
        "{}\n\nJira delivery: {}\n\n## Tasks\n{tasks}\n## Verification\nDelivery head: {}\nFinal target head: {}\nGate tree: {}\nGate command: {}\n\nThis final review PR gathers internal task contributions. A human owns colleague review and merging this PR. The fleet does not merge it.\n",
        delivery.marker(),
        delivery.jira,
        evidence.delivery_head,
        evidence.final_target_head,
        evidence.gate_tree,
        evidence.gate_command
    )
}

fn pr_value(pr: &crate::gh::DeliveryPr) -> Value {
    json!({"number": pr.number, "url": pr.url, "state": pr.state, "draft": pr.draft, "merged": pr.merged})
}

fn ensure_pr(repo: &Repo, gh: &GitHub, delivery: &Delivery) -> Result<crate::gh::DeliveryPr> {
    if let Some(pr) = discover(repo, gh, delivery)? {
        return Ok(pr);
    }
    review(repo, gh, delivery, true)?;
    discover(repo, gh, delivery)?.context(
        "the final PR is still missing after discovery; rerun afkconvoy delivery --handoff",
    )
}

fn withdraw(gh: &GitHub, pr: &crate::gh::DeliveryPr) -> Result<crate::gh::DeliveryPr> {
    if pr_terminal(pr).is_none() && !pr.draft {
        gh.set_draft(pr.number, true)?;
    }
    Ok(discover_pr(gh, pr.number)?.unwrap_or_else(|| pr.clone()))
}

fn discover_pr(gh: &GitHub, number: u64) -> Result<Option<crate::gh::DeliveryPr>> {
    Ok(gh.delivery_prs()?.into_iter().find(|pr| pr.number == number))
}

/// The part of a handoff answer that is not the delivery's identity.
struct Shown<'a> {
    ready: bool,
    reasons: Vec<String>,
    gate: Value,
    settlement: Option<&'a Settlement>,
    discrepancy: bool,
}

fn unready(reasons: Vec<String>, gate: Value) -> Shown<'static> {
    Shown { ready: false, reasons, gate, settlement: None, discrepancy: false }
}

fn handoff_answer(
    delivery: &Delivery,
    pr: Option<&crate::gh::DeliveryPr>,
    snapshot: &Snapshot,
    shown: Shown<'_>,
) -> Value {
    let delivered = shown.settlement.is_some_and(|found| found.complete) && !shown.discrepancy;
    json!({
        "jira": delivery.jira,
        "delivery": delivery.identity,
        "branch": delivery.branch,
        "final_target": delivery.final_target,
        "final_pr": pr.map(pr_value).unwrap_or(Value::Null),
        "tasks": snapshot.tasks,
        "ready": shown.ready,
        "reasons": shown.reasons,
        "gate": shown.gate,
        "settlement": shown.settlement,
        "discrepancy": shown.discrepancy,
        "delivered": delivered,
        "reviews": "not changed",
    })
}

fn handoff(repo: &Repo, gh: &GitHub, delivery: &Delivery, host: &str, now: u64) -> Result<Value> {
    delivery.validate(repo)?;
    let mut snapshot = assess(repo, gh, delivery)?;
    let mut pr = discover(repo, gh, delivery)?;
    if let Some(found) = &pr
        && let Some(settlement) = observe(repo, delivery, found, &snapshot)?
    {
        let observed = pr_terminal(found);
        let contained = on_target(repo, delivery, found, &snapshot.delivery_head)?;
        let discrepancy = settlement_differs(&settlement, &snapshot, observed, contained);
        let mut reasons = reasons_of(&snapshot.tasks);
        if !settlement.complete {
            reasons.push(match settlement.observed.as_str() {
                "closed" => "final PR closed unmerged; needs action".to_string(),
                _ if !settlement.on_target => {
                    "merged PR is not on the declared final target".to_string()
                }
                _ => "merged while attached work is incomplete; the Jira ticket is not delivered"
                    .to_string(),
            });
        }
        if discrepancy {
            reasons.push(
                "recorded settlement does not match the current delivery and was not rewritten"
                    .to_string(),
            );
        }
        return Ok(handoff_answer(
            delivery,
            pr.as_ref(),
            &snapshot,
            Shown {
                ready: false,
                reasons,
                gate: Value::Null,
                settlement: Some(&settlement),
                discrepancy: discrepancy || !settlement.complete,
            },
        ));
    }
    if !implemented(&snapshot.tasks) {
        let reasons = reasons_of(&snapshot.tasks);
        if let Some(found) = &pr {
            pr = Some(withdraw(gh, found)?);
        }
        return Ok(handoff_answer(delivery, pr.as_ref(), &snapshot, unready(reasons, Value::Null)));
    }
    if !has_diff(repo, delivery)? {
        return Ok(handoff_answer(
            delivery,
            pr.as_ref(),
            &snapshot,
            unready(vec!["no meaningful diff from the final target".to_string()], Value::Null),
        ));
    }
    let gated = aggregate(repo, delivery, host, now)?;
    let (tree, command, source) = match gated {
        Aggregate::Conflict => {
            if let Some(found) = &pr {
                pr = Some(withdraw(gh, found)?);
            }
            return Ok(handoff_answer(
                delivery,
                pr.as_ref(),
                &snapshot,
                unready(
                    vec![
                        "delivery branch conflicts with the final target; work is unchanged".into(),
                    ],
                    json!({"status": "conflict"}),
                ),
            ));
        }
        Aggregate::Red { detail } => {
            if let Some(found) = &pr {
                pr = Some(withdraw(gh, found)?);
            }
            return Ok(handoff_answer(
                delivery,
                pr.as_ref(),
                &snapshot,
                unready(vec![detail.clone()], json!({"status": "red", "detail": detail})),
            ));
        }
        Aggregate::Green { tree, command, source } => (tree, command, source),
    };
    let again = assess(repo, gh, delivery)?;
    let head_moved = again.delivery_head != snapshot.delivery_head
        || again.final_target_head != snapshot.final_target_head
        || again.tasks != snapshot.tasks;
    if head_moved || !implemented(&again.tasks) {
        snapshot = again;
        if let Some(found) = &pr {
            pr = Some(withdraw(gh, found)?);
        }
        return Ok(handoff_answer(
            delivery,
            pr.as_ref(),
            &snapshot,
            unready(
                vec!["inspected state changed before readiness was published".into()],
                json!({"status": "stale"}),
            ),
        ));
    }
    let evidence = Evidence {
        delivery: delivery.identity.clone(),
        delivery_head: snapshot.delivery_head.clone(),
        final_target_head: snapshot.final_target_head.clone(),
        tasks: snapshot.tasks.clone(),
        gate_tree: tree.clone(),
        gate_command: command.clone(),
    };
    publish(
        repo,
        &evidence_ref(repo, &delivery.identity),
        &serde_json::to_string(&evidence)?,
        true,
    )?;
    let stored = read_record::<Evidence>(repo, &evidence_ref(repo, &delivery.identity))?
        .context("handoff evidence was not recorded")?;
    if stored != evidence || !certifies(repo, &stored, &snapshot, now)? {
        if let Some(found) = &pr {
            pr = Some(withdraw(gh, found)?);
        }
        return Ok(handoff_answer(
            delivery,
            pr.as_ref(),
            &snapshot,
            unready(
                vec!["evidence no longer certifies the inspected state".into()],
                json!({"status": "stale"}),
            ),
        ));
    }
    let current = ensure_pr(repo, gh, delivery)?;
    let body = pr_body(delivery, &snapshot, &stored);
    if current.body != body {
        gh.edit_pr_body(current.number, &body)?;
    }
    let current =
        discover_pr(gh, current.number)?.context("final PR disappeared while it was updated")?;
    if current.draft {
        gh.set_draft(current.number, false)?;
    }
    let current = discover_pr(gh, current.number)?
        .context("final PR disappeared after it was marked ready")?;
    // Publication rechecks the heads it is about to certify.
    let fresh = assess(repo, gh, delivery)?;
    if fresh.delivery_head != snapshot.delivery_head
        || fresh.final_target_head != snapshot.final_target_head
        || fresh.tasks != snapshot.tasks
        || !certifies(repo, &stored, &fresh, now)?
    {
        let current = withdraw(gh, &current)?;
        return Ok(handoff_answer(
            delivery,
            Some(&current),
            &fresh,
            unready(
                vec!["inspected state changed before readiness was published".into()],
                json!({"status": "stale", "source": source}),
            ),
        ));
    }
    Ok(handoff_answer(
        delivery,
        Some(&current),
        &snapshot,
        Shown {
            ready: !current.draft && pr_terminal(&current).is_none(),
            reasons: vec![],
            gate: json!({"status": "green", "source": source, "tree": tree}),
            settlement: None,
            discrepancy: false,
        },
    ))
}

fn summary(repo: &Repo, gh: &GitHub, delivery: &Delivery, now: u64) -> Result<Value> {
    let snapshot = assess(repo, gh, delivery)?;
    let pr = discover(repo, gh, delivery)?;
    let evidence = read_record::<Evidence>(repo, &evidence_ref(repo, &delivery.identity))?;
    let certifies = match &evidence {
        Some(evidence) => certifies(repo, evidence, &snapshot, now)?,
        None => false,
    };
    let settlement = read_record::<Settlement>(repo, &settlement_ref(repo, &delivery.identity))?;
    let observed = pr.as_ref().and_then(pr_terminal);
    let contained = match &pr {
        Some(pr) => on_target(repo, delivery, pr, &snapshot.delivery_head)?,
        None => false,
    };
    let discrepancy = settlement
        .as_ref()
        .is_some_and(|found| settlement_differs(found, &snapshot, observed, contained));
    let delivered = settlement.as_ref().is_some_and(|found| found.complete) && !discrepancy;
    let published = pr.as_ref().is_some_and(|pr| !pr.draft && pr_terminal(pr).is_none());
    let detail = if delivered {
        "settled on the final target"
    } else if settlement.as_ref().is_some_and(|found| found.observed == "closed") {
        "final PR closed unmerged; needs action"
    } else if discrepancy || settlement.as_ref().is_some_and(|found| !found.complete) {
        "settlement does not match a complete delivery"
    } else if certifies && published {
        "ready for human review"
    } else if certifies {
        "aggregate evidence is green; the final PR is not published as ready"
    } else {
        "not ready"
    };
    Ok(json!({
        "delivery": delivery.identity,
        "jira": delivery.jira,
        "branch": delivery.branch,
        "final_target": delivery.final_target,
        "final_pr": pr.as_ref().map(pr_value).unwrap_or(Value::Null),
        "tasks": snapshot.tasks,
        "certifies": certifies,
        "published": published,
        "delivered": delivered,
        "discrepancy": discrepancy,
        "settlement": settlement,
        "detail": detail,
        "reasons": reasons_of(&snapshot.tasks),
    }))
}

/// Read-only delivery state for plan, doctor and the panel. It writes nothing.
/// `records` are the delivery definitions already in the look's remote listing,
/// so this does not list the remote again.
pub fn reported(repo: &Repo, now: u64, records: &[RemoteRecord]) -> Result<Vec<Value>> {
    let gh = GitHub::of(repo)?;
    let mut rows = vec![];
    for record in records {
        let id = repo
            .layout()
            .kept(&record.name)
            .and_then(|name| name.strip_prefix("delivery/"))
            .filter(|id| !id.is_empty() && !id.contains('/'))
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        match decode(repo, &id, Some(record))
            .and_then(|delivery| summary(repo, &gh, &delivery, now))
        {
            Ok(row) => rows.push(row),
            Err(error) => rows
                .push(json!({"delivery": id, "detail": format!("{error:#}"), "certifies": false})),
        }
    }
    rows.sort_by(|a, b| {
        a["delivery"].as_str().unwrap_or("").cmp(b["delivery"].as_str().unwrap_or(""))
    });
    Ok(rows)
}

pub fn panel_line(row: &Value) -> String {
    format!(
        "{} ({}) · {}",
        row["jira"].as_str().unwrap_or("delivery"),
        row["delivery"].as_str().unwrap_or(""),
        row["detail"].as_str().unwrap_or("unreadable")
    )
}

pub fn command(
    repo: &Repo,
    jira: &str,
    final_target: Option<&str>,
    issue: Option<u64>,
    handoff_requested: bool,
    now: u64,
) -> Result<Value> {
    let gh = GitHub::of(repo)?;
    let repository = gh.repository()?.to_ascii_lowercase();
    let (jira, key) = jira_reference(jira)?;
    let id = identity(&jira, &repository);
    if handoff_requested {
        ensure!(
            final_target.is_none() && issue.is_none(),
            "--handoff checks an existing delivery; omit --final-target and --issue"
        );
        let delivery = load(repo, &id)?;
        ensure!(
            delivery.repository == repository && delivery.jira == jira,
            "delivery identity conflicts with the requested Jira reference or code repo; inspect remote records"
        );
        return handoff(repo, &gh, &delivery, &crate::this_host(), now);
    }
    ensure!(
        issue.is_none() || final_target.is_some(),
        "attaching a task requires --final-target; inspect with afkconvoy delivery --jira <url>"
    );
    let delivery = match final_target {
        Some(target) => create(repo, &gh, &jira, &key, &repository, target)?,
        None => load(repo, &id)?,
    };
    ensure!(
        delivery.repository == repository && delivery.jira == jira,
        "delivery identity conflicts with the requested Jira reference or code repo; inspect remote records"
    );
    if let Some(issue) = issue {
        crate::target::attach(repo, issue, &id)?;
    }
    let review = review(repo, &gh, &delivery, final_target.is_some())?;
    let tasks = crate::target::attached(repo, &id)?;
    let mut answer = json!({"jira": delivery.jira, "delivery": id, "repository": repository,
        "record": refname(repo, &id)?, "branch": delivery.branch, "final_target": delivery.final_target,
        "final_pr": review, "tasks": tasks});
    // Inspection stays read-only. A summary that cannot be read is reported
    // here; it does not fail creation's neighbours or a quota-limited retry.
    if final_target.is_none() {
        answer["handoff"] = match summary(repo, &gh, &delivery, now) {
            Ok(row) => row,
            Err(error) => json!({ "error": format!("{error:#}") }),
        };
    }
    Ok(answer)
}

/// Covers the crash window between task publication and final PR creation.
/// Acting ticks alone call this; plans and inspection remain read-only. The
/// deliveries are those of the tick's listing of the remote (docs/adr/0043):
/// one written since is reconciled by the next tick.
pub fn reconcile(fleet: &Fleet) -> Result<()> {
    let (repo, gh) = (&fleet.repo, &fleet.gh);
    for record in fleet.records_at_open(&repo.layout().every("delivery/")) {
        let id = repo
            .layout()
            .kept(&record.name)
            .and_then(|s| s.strip_prefix("delivery/"))
            .context("invalid delivery ref; inspect origin")?
            .to_string();
        let delivery = decode(repo, &id, Some(&record))?;
        review(repo, gh, &delivery, true)?;
    }
    Ok(())
}

pub fn after_task(repo: &Repo, issue: u64, answer: &mut Value) {
    if answer["outcome"] != "merged" {
        return;
    }
    let result = (|| -> Result<Option<Value>> {
        let Some(id) = crate::target::delivery(repo, issue)? else { return Ok(None) };
        let delivery = load(repo, &id)?;
        Ok(Some(review(repo, &GitHub::of(repo)?, &delivery, true)?))
    })();
    match result {
        Ok(Some(review)) => answer["final_pr"] = review,
        Ok(None) => (),
        // A landed task stays landed. The acting tick retries the external PR.
        Err(error) => answer["delivery_error"] = json!(format!("{error:#}")),
    }
}

pub fn brief(repo: &Repo, issue: u64) -> Result<String> {
    let Some(id) = crate::target::delivery(repo, issue)? else { return Ok(String::new()) };
    let d = load(repo, &id)?;
    Ok(format!(
        "\n## Jira delivery\nThis task contributes to delivery `{id}` ({}) on `{}`. Add `Delivery: {id}` to the internal task PR body alongside its task-closing line. The final review PR targets `{}` and is owned by a human; complete only this task's internal PR and landing.\n\nOpen follow-up task issues without ready or flow labels. Run `afkconvoy intake --issue <n> --from {issue}` (add `--workflow <name>` in a bound repo); intake inherits this delivery before making the task dispatchable. For discussion creation run `afkconvoy flows --delivery {id}` and follow its line then intake_line. Work belonging to another delivery must name `--delivery <other-id>` explicitly in its code repo. Dependencies and parent links alone never assign delivery membership.\n",
        d.jira, d.branch, d.final_target
    ))
}

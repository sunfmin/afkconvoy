//! Jira references group task contributions; immutable remote records own
//! branch policy, while GitHub owns the human-reviewed final PR.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::{REMOTE, RemoteRecord, Repo};

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
    let rows = repo.remote_records(&name)?;
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
            read = repo.remote_records(&name)?;
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
        repo.try_git(&["check-ref-format", &format!("refs/heads/{}", delivery.final_target)])?.ok,
        "invalid final target in delivery {id}; restore its immutable record"
    );
    Ok(delivery)
}

pub(crate) fn load(repo: &Repo, id: &str) -> Result<Delivery> {
    let name = refname(repo, id)?;
    let rows = repo.remote_records(&name)?;
    decode(repo, id, rows.first())
}

impl Delivery {
    fn validate(&self, repo: &Repo) -> Result<()> {
        let tip = repo.fetched(&format!("refs/heads/{}", self.branch))?.with_context(|| {
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
            repo.remote_sha(&format!("refs/heads/{}", self.final_target))?.is_some(),
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
    let existing = repo.remote_records(&name)?;
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
        repo.try_git(&["check-ref-format", &format!("refs/heads/{final_target}")])?.ok
            && !final_pr(final_target, "")
            && !final_target.starts_with("convoy-"),
        "invalid final target {final_target:?}; name an existing code branch on origin"
    );
    let initial = repo.fetched(&format!("refs/heads/{final_target}"))?
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
    let pushed = crate::publication::push(
        repo,
        &[
            crate::publication::Update { name: &name, old: &zero, new: &sha },
            crate::publication::Update { name: &branch_ref, old: &zero, new: &delivery.initial },
            crate::publication::Update {
                name: &target_ref,
                old: &delivery.initial,
                new: &delivery.initial,
            },
        ],
    );
    // Lost receipts and competing creators both discover the actual remote.
    let records = repo.remote_records(&name)?;
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
    let rows = repo.remote_records(&name)?;
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
    let pushed = repo.try_git(&["push", "--quiet", REMOTE, &format!("{sha}:{name}")])?;
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
        .fetched(&format!("refs/heads/{}", delivery.final_target))?
        .context("final target disappeared; restore it before retrying")?;
    let head = repo
        .fetched(&format!("refs/heads/{}", delivery.branch))?
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
            json!({"number": pr.number, "url": pr.url, "state": pr.state, "draft": pr.draft}),
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
    Ok(json!({"number": pr.number, "url": pr.url, "state": pr.state, "draft": pr.draft}))
}

pub fn command(
    repo: &Repo,
    jira: &str,
    final_target: Option<&str>,
    issue: Option<u64>,
) -> Result<Value> {
    ensure!(
        issue.is_none() || final_target.is_some(),
        "attaching a task requires --final-target; inspect with afkconvoy delivery --jira <url>"
    );
    let gh = GitHub::of(repo)?;
    let repository = gh.repository()?.to_ascii_lowercase();
    let (jira, key) = jira_reference(jira)?;
    let id = identity(&jira, &repository);
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
    Ok(json!({"jira": delivery.jira, "delivery": id, "repository": repository,
        "record": refname(repo, &id)?, "branch": delivery.branch, "final_target": delivery.final_target,
        "final_pr": review, "tasks": tasks}))
}

/// Covers the crash window between task publication and final PR creation.
/// Acting ticks alone call this; plans and inspection remain read-only.
pub fn reconcile(repo: &Repo, gh: &GitHub) -> Result<()> {
    for record in repo.remote_records(&repo.layout().every("delivery/"))? {
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

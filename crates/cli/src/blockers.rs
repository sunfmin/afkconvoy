//! Blockers nobody on this host will work. Native edges carry their repo
//! and labels in the paged read, so classification needs no per-issue read.

use std::path::PathBuf;

use afkconvoy_core::line::Line;
use afkconvoy_core::plan::UnworkedBlocker;
use afkconvoy_core::tracker::{Tracker, repo_labels};
use anyhow::Result;

use crate::checkout_list::{List, named_repo, repo_of};
use crate::gh::{BlockedBy, GitHub, OpenIssues};
use crate::git::Repo;

struct Checkout {
    repo: String,
    path: PathBuf,
}

fn name(repo: &Repo) -> Result<String> {
    named_repo(repo).map(|name| name.to_string()).or_else(|_| repo_of(repo))
}

/// The checkouts serving this tracker, including the checkout being asked
/// about even before its first tick has put it on the checkout list.
fn serving(repo: &Repo, tracker: &str) -> Result<Vec<Checkout>> {
    let mut found = vec![Checkout { repo: name(repo)?, path: repo.main_checkout()? }];
    for entry in List::of_host()?.read()?.checkouts {
        let Ok(other) = Repo::at(&entry.checkout) else { continue };
        let Ok(name) = name(&other) else { continue };
        let uses = entry.tracker.as_ref().map_or(name.as_str(), |name| name.as_str());
        if uses.eq_ignore_ascii_case(tracker) && !found.iter().any(|row| row.repo == name) {
            found.push(Checkout { repo: name, path: entry.checkout });
        }
    }
    Ok(found)
}

/// Classify only open edges whose ownership GitHub supplied. A legacy read
/// without ownership is not evidence that no fleet works its blocker.
pub fn unworked(
    repo: &Repo,
    tracker: &Tracker,
    edges: &[(u64, Vec<BlockedBy>)],
) -> Result<Vec<UnworkedBlocker>> {
    if edges.iter().all(|(_, edges)| edges.is_empty()) {
        return Ok(vec![]);
    }
    let own = name(repo)?;
    let named = tracker.named().map_or(own.as_str(), |name| name.as_str());
    let checkouts = serving(repo, named)?;
    let mut found = vec![];
    for (issue, edges) in edges {
        for edge in edges.iter().filter(|edge| edge.state.eq_ignore_ascii_case("open")) {
            let of = edge.repo.as_deref().unwrap_or(named);
            let outside = !of.eq_ignore_ascii_case(named);
            let labels = edge.labels.as_deref().map(repo_labels);
            let target = if outside {
                checkouts.iter().find(|checkout| checkout.repo.eq_ignore_ascii_case(of))
            } else {
                labels.as_ref().filter(|labels| labels.len() == 1).and_then(|labels| {
                    checkouts.iter().find(|checkout| {
                        labels[0].eq_ignore_ascii_case(&format!("repo:{}", checkout.repo))
                    })
                })
            };
            // In a repo that is its own tracker, unlabelled issues are its
            // own. Preserve that ordinary, supported way of working.
            let own_issue = !outside
                && matches!(tracker, Tracker::Own)
                && labels.as_ref().is_some_and(Vec::is_empty);
            if !outside && (labels.is_none() || target.is_some() || own_issue) {
                continue;
            }
            let reason = if outside {
                format!("outside the tracker {named}")
            } else {
                format!("no single repo: label names a checkout serving the tracker {named}")
            };
            let path = target
                .map(|checkout| checkout.path.display().to_string())
                .unwrap_or_else(|| "<target-checkout>".to_string());
            let label =
                target.map(|checkout| format!("repo:{}", checkout.repo)).unwrap_or_else(|| {
                    if outside { format!("repo:{of}") } else { "repo:<owner>/<repo>".to_string() }
                });
            let ready = target
                .and_then(|checkout| {
                    crate::config::load(&checkout.path.join(crate::config::FILE)).ok()
                })
                .filter(|config| config.bound.is_none())
                .map(|config| config.ready_label)
                .unwrap_or_else(|| "<flow_label from afkconvoy flows>".to_string());
            let transfer = if outside {
                Line::new()
                    .fixed("gh issue transfer ")
                    .number(edge.number)
                    .fixed(" ")
                    .word(named)
                    .fixed(" --repo ")
                    .word(of)
                    .fixed("\n")
                    .own()
            } else {
                String::new()
            };
            let flows = Line::new().fixed("afkconvoy flows ").word(&path).own();
            // Construct the repair independently of the number GitHub will
            // assign on transfer: that number cannot be known beforehand.
            let label_line = Line::new().fixed("gh issue edit ");
            let label_line = if outside {
                label_line.fixed("<number-after-transfer>")
            } else {
                label_line.number(edge.number)
            };
            let mut label_line = label_line.fixed(" --repo ").word(named);
            for old in labels.iter().flatten().filter(|old| !old.eq_ignore_ascii_case(&label)) {
                label_line = label_line.fixed(" --remove-label ").word(old);
            }
            let label_line = label_line
                .fixed(" --add-label ")
                .word(&label)
                .fixed(" --add-label ")
                .word(&ready)
                .own();
            found.push(UnworkedBlocker {
                issue: *issue,
                blocker: edge.number,
                repo: of.to_string(),
                reason,
                fix: format!("{transfer}{flows}\n{label_line}"),
            });
        }
    }
    found.sort_by(|a, b| (a.issue, &a.repo, a.blocker).cmp(&(b.issue, &b.repo, b.blocker)));
    Ok(found)
}

pub fn read(repo: &Repo, tracker: &Tracker, open: &OpenIssues) -> Result<Vec<UnworkedBlocker>> {
    let edges: Vec<_> =
        open.blockers.iter().map(|(&issue, edges)| (issue, edges.clone())).collect();
    unworked(repo, tracker, &edges)
}

/// Whether every open edge is unworked. Compare repo identity too: an
/// outside blocker and a served blocker may have exactly the same number.
pub fn can_hand_on(issue: u64, edges: &[BlockedBy], rows: &[UnworkedBlocker]) -> bool {
    edges.iter().filter(|edge| edge.state.eq_ignore_ascii_case("open")).all(|edge| {
        rows.iter().any(|row| {
            row.issue == issue
                && row.blocker == edge.number
                && edge.repo.as_ref().is_some_and(|repo| repo.eq_ignore_ascii_case(&row.repo))
        })
    })
}

/// A fresh read for a consult, shared by its claim check and its brief.
pub fn for_issue(repo: &Repo, gh: &GitHub, issue: u64) -> Result<Vec<UnworkedBlocker>> {
    unworked(repo, &gh.tracker, &[(issue, gh.blocked_by(issue)?)])
}

pub fn evidence(rows: &[UnworkedBlocker]) -> String {
    rows.iter()
        .map(|row| format!("{}#{}: {}; repair:\n{}", row.repo, row.blocker, row.reason, row.fix))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn findings(rows: &[UnworkedBlocker]) -> Vec<crate::doctor::Finding> {
    rows.iter()
        .map(|row| {
            crate::doctor::finding(
                crate::doctor::Kind::BlockerUnworked,
                serde_json::json!(["blocker", row.issue, row.repo, row.blocker]),
                format!(
                    "issue #{} waits on a blocker no fleet works: {}#{}",
                    row.issue, row.repo, row.blocker
                ),
                row.reason.clone(),
                row.fix.clone(),
            )
        })
        .collect()
}

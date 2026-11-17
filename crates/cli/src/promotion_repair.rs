//! A red promotion is fixed forward by the proving branch's own worker
//! (docs/adr/0090): the PRs' workers have ended and their issues are closed,
//! so the fleet opens one repair issue for the red, which its repair worker
//! takes as it takes any repair. A second red while that issue is open is
//! added to it; the next green promotion closes it.
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::{REPAIR_LABEL, is_repair};
use anyhow::Result;

use crate::git::Repo;
use crate::issues::Issues;

/// The first line of a promotion repair issue's body: what finds it again.
fn marker(proving: &str) -> String {
    format!("<!--convoy:promotion-repair branch={proving}-->")
}

/// Whether `body` is the body of a promotion repair issue of `proving`.
pub fn is_of(body: &str, proving: &str) -> bool {
    body.starts_with(&marker(proving))
}

/// The open promotion repair issue of `proving`, if one stands.
pub fn open(issues: &dyn Issues, proving: &str) -> Result<Option<u64>> {
    for (number, _) in issues.carrying(REPAIR_LABEL)? {
        let read = issues.issue(number)?;
        if !read.is_closed() && read.body.as_deref().is_some_and(|body| is_of(body, proving)) {
            return Ok(Some(number));
        }
    }
    Ok(None)
}

/// What a red promotion reports to the repair worker.
pub struct Red<'a> {
    pub proving: &'a str,
    pub tip: &'a str,
    pub said: &'a str,
    pub failing: &'a [String],
    pub output: &'a std::path::Path,
    /// The gate's output, its tail.
    pub excerpt: &'a str,
    /// Every issue whose PR landed on the proving branch since the last
    /// promotion, with its PR.
    pub landed: &'a [(Option<u64>, u64)],
    pub gate_command: &'a str,
}

fn report(red: &Red) -> String {
    let mut text =
        format!("The proving branch `{}` at `{}`: {}\n\n", red.proving, red.tip, red.said);
    text.push_str("Failing tests:\n");
    if red.failing.is_empty() {
        text.push_str("- (the gate's output names none)\n");
    }
    for test in red.failing {
        text.push_str(&format!("- `{test}`\n"));
    }
    text.push_str(&format!(
        "\nGate output (`{}`, on the host):\n\n```\n{}\n```\n",
        red.output.display(),
        red.excerpt
    ));
    text.push_str("\nLanded on the proving branch since the last promotion:\n");
    if red.landed.is_empty() {
        text.push_str("- (none read)\n");
    }
    for (issue, pr) in red.landed {
        match issue {
            Some(issue) => text.push_str(&format!("- #{issue}, PR #{pr}\n")),
            None => text.push_str(&format!("- PR #{pr}\n")),
        }
    }
    text
}

/// A red promotion: opens the repair issue, or adds this red to the open one →
/// its number.
pub fn red(repo: &Repo, config: &Config, red: &Red) -> Result<u64> {
    let gh = crate::gh::GitHub::of(repo)?;
    let issues = crate::issues::of(&gh);
    let facts = report(red);
    if let Some(number) = open(&*issues, red.proving)? {
        issues
            .comment(number, &format!("**afkconvoy** — the promotion is red again.\n\n{facts}"))?;
        return Ok(number);
    }
    let labels = issues.labels()?;
    for label in [config.ready_label.as_str(), REPAIR_LABEL] {
        if !labels.iter().any(|known| known == label) {
            issues.create_label(label, "afkconvoy: repair the red proving branch")?;
        }
    }
    let body = format!(
        "{}\nA promotion of the proving branch `{}` was gated and is red. Its PRs' workers have \
         ended and their issues are closed: you fix it forward.\n\nReproduce at `{}` with `{}`. \
         Find the cause from the gate's output, fix it, and add a regression test. Branch from \
         the proving tip, pass the fast gate, open the PR against `{}` and run `afkconvoy land \
         --issue <n>`. The next green promotion closes this issue. Fast-green PRs keep landing \
         meanwhile; only the promotion waits.\n\n{facts}",
        marker(red.proving),
        red.proving,
        red.tip,
        red.gate_command,
        red.proving,
    );
    let title = format!(
        "Repair the red promotion of {} at {}",
        red.proving,
        &red.tip[..12.min(red.tip.len())]
    );
    let number = issues.open_issue(&title, &body, &[])?;
    for label in [config.ready_label.as_str(), REPAIR_LABEL] {
        issues.add_label(number, label)?;
    }
    Ok(number)
}

/// A green promotion: closes the repair issue its proving branch has open →
/// its number.
pub fn green(repo: &Repo, proving: &str, tip: &str) -> Result<Option<u64>> {
    let gh = crate::gh::GitHub::of(repo)?;
    let issues = crate::issues::of(&gh);
    let Some(number) = open(&*issues, proving)? else { return Ok(None) };
    issues.comment(
        number,
        &format!("**afkconvoy** — a promotion of `{proving}` at `{tip}` is green: this repair is promoted."),
    )?;
    issues.close_issue(number)?;
    Ok(Some(number))
}

/// Whether `labels` and `body` are a promotion repair issue of any branch.
pub fn is_any(labels: &[String], body: Option<&str>) -> bool {
    is_repair(labels) && body.is_some_and(|b| b.starts_with("<!--convoy:promotion-repair "))
}

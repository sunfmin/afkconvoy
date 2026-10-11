//! GitHub, through `gh` as a subprocess run in the checkout: `gh` works out
//! which repository that is, so nothing here names one.

use std::path::PathBuf;
use std::process::Command;

use afkconvoy_core::plan::{Issue, PullRequest};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// What is read of every open issue. `gh api` fills `{owner}` and `{repo}` in.
pub const ISSUES_JQ: &str = ".[] | select(.pull_request == null) | {number, title, \
    labels: [.labels[].name], blocked_by: (.issue_dependencies_summary.blocked_by // 0)}";
/// What is read of one issue, open or not.
pub const ISSUE_JQ: &str = "{id, title, state, state_reason, labels: [.labels[].name], \
    pull_request: (.pull_request != null)}";
/// What is read of each comment on an issue or a PR.
pub const COMMENTS_JQ: &str = ".[] | {id, body, url: .html_url}";
pub const PR_FIELDS: &str = "number,title,headRefName,headRefOid,body,closingIssuesReferences";

pub struct GitHub {
    pub dir: PathBuf,
}

/// One issue, open or closed, as `GitHub::issue` reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct IssueRead {
    /// GitHub's database id: what a dependency edge names an issue by.
    pub id: u64,
    pub title: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub pull_request: bool,
}

/// One comment on an issue or a pull request.
#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub url: String,
}

impl IssueRead {
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
}

impl GitHub {
    /// One `gh` call that must succeed → its stdout.
    pub fn gh(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("gh")
            .current_dir(&self.dir)
            .args(args)
            .output()
            .context("gh could not run")?;
        if !out.status.success() {
            let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
            bail!("gh {what} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Every open issue — all of them, page after page.
    pub fn open_issues(&self) -> Result<Vec<Issue>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            title: String,
            labels: Vec<String>,
            blocked_by: u64,
        }
        let endpoint = "repos/{owner}/{repo}/issues?state=open&per_page=100";
        let out = self.gh(&["api", "--paginate", endpoint, "--jq", ISSUES_JQ])?;
        let mut issues = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            issues.push(Issue {
                number: row.number,
                title: row.title,
                labels: row.labels,
                open_blockers: row.blocked_by,
            });
        }
        Ok(issues)
    }

    /// Every open pull request.
    pub fn open_prs(&self) -> Result<Vec<PullRequest>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            number: u64,
            title: Option<String>,
            head_ref_name: String,
            head_ref_oid: String,
            body: Option<String>,
            closing_issues_references: Option<Vec<Linked>>,
        }
        #[derive(Deserialize)]
        struct Linked {
            number: u64,
        }
        let out =
            self.gh(&["pr", "list", "--state", "open", "--limit", "1000", "--json", PR_FIELDS])?;
        let rows: Vec<Row> = serde_json::from_str(&out).context("gh pr list: unexpected answer")?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let linked: Vec<u64> = row
                    .closing_issues_references
                    .unwrap_or_default()
                    .iter()
                    .map(|l| l.number)
                    .collect();
                PullRequest {
                    number: row.number,
                    title: row.title.unwrap_or_default(),
                    head_branch: row.head_ref_name,
                    head_sha: row.head_ref_oid,
                    closes: PullRequest::closing(row.body.as_deref().unwrap_or(""), &linked),
                }
            })
            .collect())
    }

    /// One issue, open or closed. An error when it cannot be read.
    pub fn issue(&self, number: u64) -> Result<IssueRead> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}");
        let out = self.gh(&["api", &endpoint, "--jq", ISSUE_JQ])?;
        serde_json::from_str(&out)
            .with_context(|| format!("gh api issue {number}: unexpected answer"))
    }

    /// Every comment on an issue or a PR, oldest first.
    pub fn comments(&self, number: u64) -> Result<Vec<Comment>> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments?per_page=100");
        let out = self.gh(&["api", "--paginate", &endpoint, "--jq", COMMENTS_JQ])?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api comments: unexpected answer"))
            .collect()
    }

    /// Posts one comment on an issue or a PR.
    pub fn comment(&self, number: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments");
        self.gh(&["api", "--method", "POST", &endpoint, "-f", &format!("body={body}")])?;
        Ok(())
    }

    /// Closes one issue as completed.
    pub fn close_issue(&self, number: u64) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}");
        self.gh(&["api", "--method", "PATCH", &endpoint, "-f", "state=closed", "--silent"])?;
        Ok(())
    }

    /// Closes one PR without merging it, with a comment saying why.
    pub fn close_pr(&self, number: u64, comment: &str) -> Result<()> {
        self.gh(&["pr", "close", &number.to_string(), "--comment", comment])?;
        Ok(())
    }
}

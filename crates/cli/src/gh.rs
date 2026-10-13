//! GitHub, through `gh` as a subprocess run in the checkout: `gh` works out
//! which repository that is, so nothing here names one.

use std::path::PathBuf;
use std::process::Command;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::layout::{Guard, Protection};
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
/// What is read of each issue another is blocked by.
pub const BLOCKED_BY_JQ: &str = ".[] | {number, state}";
/// What is read of each label a repo has.
pub const LABELS_JQ: &str = ".[].name";
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

/// One issue another is recorded as blocked by — a native dependency edge.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockedBy {
    pub number: u64,
    pub state: String,
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

    /// The commit at the head of the default branch of `repo`.
    pub fn head_commit(&self, repo: &RepoName) -> Result<String> {
        let out = self.gh(&["api", &format!("repos/{repo}/commits/HEAD"), "--jq", ".sha"])?;
        let commit = out.trim();
        let plain = !commit.is_empty() && commit.bytes().all(|b| b.is_ascii_hexdigit());
        if !plain {
            bail!("gh api: {repo} has no commit at the head of its default branch");
        }
        Ok(commit.to_string())
    }

    /// Clones the head of `repo`'s default branch, and nothing behind it, into
    /// the bare repo `into`. `gh` picks the protocol the user set it up with.
    pub fn clone_head(&self, repo: &RepoName, into: &std::path::Path) -> Result<()> {
        let into = into.to_string_lossy();
        self.gh(&[
            "repo",
            "clone",
            repo.as_str(),
            &into,
            "--",
            "--bare",
            "--depth",
            "1",
            "--quiet",
        ])?;
        Ok(())
    }

    /// The protection of one branch, as far as it can be read. A branch with
    /// none is a read that succeeded; only a read GitHub refused, or one
    /// nobody can parse, is unreadable.
    pub fn protection(&self, branch: &str) -> Guard {
        let endpoint = format!("repos/{{owner}}/{{repo}}/branches/{branch}/protection");
        let out = match Command::new("gh").current_dir(&self.dir).args(["api", &endpoint]).output()
        {
            Ok(out) => out,
            Err(error) => return Guard::Unreadable(format!("gh could not run: {error}")),
        };
        let (stdout, stderr) =
            (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            let said = format!("{stderr}{stdout}");
            return if said.contains("Branch not protected") {
                Guard::Unprotected
            } else {
                Guard::Unreadable(stderr.trim().to_string())
            };
        }
        #[derive(Deserialize)]
        struct Check {
            context: String,
        }
        #[derive(Deserialize, Default)]
        struct Checks {
            #[serde(default)]
            contexts: Vec<String>,
            #[serde(default)]
            checks: Vec<Check>,
        }
        #[derive(Deserialize)]
        struct Switch {
            enabled: bool,
        }
        #[derive(Deserialize)]
        struct Rules {
            required_status_checks: Option<Checks>,
            required_pull_request_reviews: Option<serde_json::Value>,
            restrictions: Option<serde_json::Value>,
            lock_branch: Option<Switch>,
        }
        let Ok(rules) = serde_json::from_str::<Rules>(&stdout) else {
            return Guard::Unreadable("GitHub's answer could not be parsed".to_string());
        };
        let checks = rules.required_status_checks.unwrap_or_default();
        let mut required = checks.contexts;
        required.extend(checks.checks.into_iter().map(|check| check.context));
        required.sort();
        required.dedup();
        Guard::Protected(Protection::new(
            required,
            rules.required_pull_request_reviews.is_some(),
            rules.restrictions.is_some(),
            rules.lock_branch.is_some_and(|lock| lock.enabled),
        ))
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

    /// Replaces the body of one comment, on an issue or a PR.
    pub fn edit_comment(&self, id: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/comments/{id}");
        self.gh(&[
            "api",
            "--method",
            "PATCH",
            &endpoint,
            "-f",
            &format!("body={body}"),
            "--silent",
        ])?;
        Ok(())
    }

    /// Opens an issue carrying `labels` → its number.
    pub fn open_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64> {
        let (title, body) = (format!("title={title}"), format!("body={body}"));
        let labels: Vec<String> = labels.iter().map(|label| format!("labels[]={label}")).collect();
        let mut args = vec![
            "api",
            "--method",
            "POST",
            "repos/{owner}/{repo}/issues",
            "-f",
            &title,
            "-f",
            &body,
        ];
        for label in &labels {
            args.extend(["-f", label.as_str()]);
        }
        args.extend(["--jq", ".number"]);
        let out = self.gh(&args)?;
        out.trim().parse().with_context(|| format!("gh api issues: no number in {out:?}"))
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

    /// The open PRs of another repo, `repo`: the number and the head branch
    /// of each.
    pub fn open_prs_of(&self, repo: &RepoName) -> Result<Vec<(u64, String)>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            number: u64,
            head_ref_name: String,
        }
        let fields = "number,headRefName";
        let out = self.gh(&[
            "pr",
            "list",
            "--repo",
            repo.as_str(),
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            fields,
        ])?;
        let rows: Vec<Row> = serde_json::from_str(&out).context("gh pr list: unexpected answer")?;
        Ok(rows.into_iter().map(|row| (row.number, row.head_ref_name)).collect())
    }

    /// What PR `number` of another repo, `repo`, changes, as `git diff`
    /// writes it.
    pub fn pr_diff_of(&self, repo: &RepoName, number: u64) -> Result<String> {
        self.gh(&["pr", "diff", &number.to_string(), "--repo", repo.as_str()])
    }

    /// Closes one PR of another repo, `repo`, without merging it, with a
    /// comment saying why.
    pub fn close_pr_of(&self, repo: &RepoName, number: u64, comment: &str) -> Result<()> {
        let number = number.to_string();
        self.gh(&["pr", "close", &number, "--repo", repo.as_str(), "--comment", comment])?;
        Ok(())
    }

    /// The issues GitHub records `number` as blocked by, open or closed.
    pub fn blocked_by(&self, number: u64) -> Result<Vec<BlockedBy>> {
        let endpoint = format!(
            "repos/{{owner}}/{{repo}}/issues/{number}/dependencies/blocked_by?per_page=100"
        );
        let out = self.gh(&["api", "--paginate", &endpoint, "--jq", BLOCKED_BY_JQ])?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api blocked_by: unexpected answer"))
            .collect()
    }

    /// The open issues among those `number` is blocked by.
    pub fn open_blockers(&self, number: u64) -> Result<Vec<u64>> {
        let open = self.blocked_by(number)?.into_iter().filter(|edge| edge.state == "open");
        Ok(open.map(|edge| edge.number).collect())
    }

    /// Records `number` as blocked by the issue whose database id is `blocker_id`.
    pub fn add_blocker(&self, number: u64, blocker_id: u64) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/dependencies/blocked_by");
        let field = format!("issue_id={blocker_id}");
        self.gh(&["api", "--method", "POST", &endpoint, "-F", &field, "--silent"])?;
        Ok(())
    }

    /// Every label the repo has, by name.
    pub fn labels(&self) -> Result<Vec<String>> {
        let endpoint = "repos/{owner}/{repo}/labels?per_page=100";
        let out = self.gh(&["api", "--paginate", endpoint, "--jq", LABELS_JQ])?;
        Ok(out.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
    }

    /// Creates the label `name` on the repo, which has none of that name, to
    /// be shown with `description`.
    pub fn create_label(&self, name: &str, description: &str) -> Result<()> {
        let (name, description) = (format!("name={name}"), format!("description={description}"));
        let endpoint = "repos/{owner}/{repo}/labels";
        self.gh(&[
            "api",
            "--method",
            "POST",
            endpoint,
            "-f",
            &name,
            "-f",
            &description,
            "-f",
            "color=1d76db",
            "--silent",
        ])?;
        Ok(())
    }

    /// Puts one label on an issue; GitHub creates a label the repo lacks.
    pub fn add_label(&self, number: u64, label: &str) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/labels");
        let field = format!("labels[]={label}");
        self.gh(&["api", "--method", "POST", &endpoint, "-f", &field, "--silent"])?;
        Ok(())
    }

    /// Takes one label off an issue that carries it.
    pub fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let label = crate::judge::label_segment(label);
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/labels/{label}");
        self.gh(&["api", "--method", "DELETE", &endpoint, "--silent"])?;
        Ok(())
    }
}

//! The landing train on the remote, and a worker joining its PR to it.

use afkconvoy_core::config::Config;
use afkconvoy_core::lock::{keeps_off, named};
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::train::{
    JoinStep, Joined, Line, Logged, PUSH_TRIES, join_message, join_steps, joins_onto, on_train,
    read_joins, train_refs,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::{Listing, REMOTE, Repo};
use crate::lock::standing_locks;

/// The landing train as one read of the remote saw it, its commits in the
/// repo that asked.
pub struct Train {
    /// What the next PR joins onto: the line's tip, or the base branch's when
    /// the line holds nothing the base branch lacks.
    pub tip: String,
    /// The base branch's tip.
    pub base: String,
}

impl Train {
    /// The train as the remote has it now.
    pub fn read(repo: &Repo, config: &Config) -> Result<Train> {
        Train::listed(repo, config, &repo.listing(&config.base_branch)?)
    }

    /// The train as `listing` saw it.
    pub fn listed(repo: &Repo, config: &Config, listing: &Listing) -> Result<Train> {
        let branch = &config.base_branch;
        let base =
            listing.base.clone().ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
        repo.bring(&base, &listing.base_ref)?;
        let line_ref = train_refs(repo.layout(), branch).line;
        let line = listing.sha(&line_ref).map(str::to_string);
        if let Some(tip) = &line {
            repo.bring(tip, &line_ref)?;
        }
        let on_base = match &line {
            Some(tip) => repo.within(tip, &base)?,
            None => false,
        };
        let line = line.as_deref().map(|tip| Line { tip, on_base });
        let tip = joins_onto(line, &base).to_string();
        Ok(Train { tip, base })
    }

    /// The joinings on the train that the base branch does not hold yet,
    /// oldest first.
    pub fn joins(&self, repo: &Repo) -> Result<Vec<Joined>> {
        if self.tip == self.base {
            return Ok(vec![]);
        }
        joins_in(repo, &[&format!("{}..{}", self.base, self.tip)])
    }
}

/// The joinings among the commits `revs` selects, oldest first. Whatever
/// narrows the walk (`--grep`) never decides what a commit is.
pub fn joins_in(repo: &Repo, revs: &[&str]) -> Result<Vec<Joined>> {
    let mut args = vec!["log", "--reverse", "--merges", "--format=%H%x09%P%x09%B%x00"];
    args.extend(revs);
    let log = repo.git_raw(&args)?;
    let commits: Vec<Logged> = log
        .split('\0')
        .filter_map(|entry| {
            let mut parts = entry.trim_matches('\n').splitn(3, '\t');
            Some(Logged { sha: parts.next()?, parents: parts.next()?, message: parts.next()? })
        })
        .collect();
    Ok(read_joins(&commits))
}

/// The files a merge in progress in a worktree has left conflicted.
pub fn unmerged(repo: &Repo) -> Result<Vec<String>> {
    let listed = repo.git(&["diff", "--name-only", "--diff-filter=U"])?;
    Ok(listed.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
}

/// Refuses a worktree whose tracked files differ from its commit: what would
/// join the train is not what the worker has there.
pub fn require_committed(repo: &Repo) -> Result<()> {
    let dirty = repo.git_raw(&["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        bail!(
            "the worktree {} has uncommitted changes to tracked files — commit them (a resolved \
             conflict must be committed) or discard them:\n{}",
            repo.dir.display(),
            dirty.trim_end()
        );
    }
    Ok(())
}

/// Merges the commit `sha` into the branch checked out in a worktree — never a
/// rebase → the conflicted files, none when the merge is clean. A conflict is
/// left in place for the worker to resolve and commit; one already in progress
/// is reported as it stands. `what` names `sha` in an error.
pub fn merge_in(repo: &Repo, sha: &str, what: &str) -> Result<Vec<String>> {
    let files = unmerged(repo)?;
    if !files.is_empty() {
        return Ok(files);
    }
    require_committed(repo)?;
    let merged = repo.try_git(&["merge", "--no-edit", "--quiet", sha])?;
    if merged.ok {
        return Ok(vec![]);
    }
    let files = unmerged(repo)?;
    if files.is_empty() {
        let said = if merged.stderr.trim().is_empty() { &merged.stdout } else { &merged.stderr };
        bail!("git merge of {what} into {} failed: {}", repo.dir.display(), said.trim());
    }
    Ok(files)
}

/// `head` merged onto `base` as ONE merge commit whose second parent is `head`
/// → its sha, `None` when the two conflict. Made in the object store alone: no
/// worktree is touched and no branch moves, and a head that changes nothing
/// against `base` still gets its commit — which is what says the PR joined.
fn merged_onto(
    repo: &Repo,
    base: &str,
    head: &str,
    message: &str,
    now: u64,
) -> Result<Option<String>> {
    let merged = repo.try_git(&["merge-tree", "--write-tree", base, head])?;
    let tree = match merged.stdout.split_whitespace().next() {
        Some(tree) if merged.ok => tree,
        // git exits 1 for a merge that conflicts, and names the tree it got to
        Some(_) => return Ok(None),
        None => bail!("git merge-tree of {head} onto {base} failed: {}", merged.stderr.trim()),
    };
    let at = format!("@{now} +0000");
    let dated = [("GIT_AUTHOR_DATE", at.as_str()), ("GIT_COMMITTER_DATE", at.as_str())];
    let made =
        repo.try_git_with(&["commit-tree", tree, "-p", base, "-p", head, "-m", message], &dated)?;
    if !made.ok {
        bail!("could not commit the merge of {head} onto {base}: {}", made.stderr.trim());
    }
    Ok(Some(made.stdout))
}

/// The head of a PR's `branch` on the remote, with a worktree that is only
/// BEHIND it caught up to it — a worktree recreated before the last push. One
/// that is ahead (a resolution, a fix: the join pushes them) or in the middle
/// of a merge is left as it is.
fn own_head(repo: &Repo, branch: &str) -> Result<String> {
    let tip = repo
        .fetched(&format!("refs/heads/{branch}"))?
        .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
    let here = repo.git(&["rev-parse", "HEAD"])?;
    let merging = repo.try_git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])?.ok;
    if here != tip && !merging && repo.within(&here, &tip)? {
        repo.git(&["merge", "--ff-only", "--quiet", &tip])?;
    }
    Ok(tip)
}

const RESOLVE: &str = "resolve every file, `git add` it, COMMIT the merge, run the gate again, \
                       and run this again";

/// `afkconvoy land --issue <n>`: a worker puts its own PR on the landing
/// train, from the worktree it is called in → `joined`, `conflict`, or
/// `merged` when the base branch already holds the PR's head. No gate runs
/// here: the train's own worker gates the train and lands it.
///
/// With a fleet lock standing, the PR of an issue that is not one joins
/// nothing → `locked`, and nothing is pushed.
///
/// The push of the train's tip is the only lock: refused — another PR joined
/// first — the join is made again on the new tip. Nothing on the train is
/// rewritten, so a PR that conflicts with it resolves that once.
pub fn land(repo: &Repo, config: &Config, gh: &GitHub, issue: u64, now: u64) -> Result<Value> {
    let prs = gh.open_prs()?;
    let Some(pr) = closing_pr(&prs, issue) else {
        bail!(
            "no open PR closes issue #{issue} — there is nothing to land (if it has already \
             merged, stop)"
        );
    };
    let line = train_refs(repo.layout(), &config.base_branch).line;
    let mut answer = json!({"issue": issue, "pr": pr.number});
    let mut stop = |outcome: &str, more: Value| {
        answer["outcome"] = json!(outcome);
        for (key, value) in more.as_object().into_iter().flatten() {
            answer[key] = value.clone();
        }
        answer.clone()
    };
    let conflict = |files: Vec<String>, detail: String| json!({"files": files, "worktree": repo.dir, "detail": detail});
    let locks = standing_locks(gh, config)?;
    if keeps_off(&locks, issue) {
        let detail = format!(
            "a fleet lock stands ({}): no PR of another issue joins the landing train until it \
             has ended — stop, and run this again when you are told to carry on",
            named(&locks)
        );
        return Ok(stop("locked", json!({"locks": locks, "detail": detail})));
    }
    let mut pushed = own_head(repo, &pr.head_branch)?;

    for _ in 0..PUSH_TRIES {
        let files = unmerged(repo)?;
        if !files.is_empty() {
            let detail = format!(
                "a merge of the landing train into this branch is in progress here — {RESOLVE}"
            );
            return Ok(stop("conflict", conflict(files, detail)));
        }
        require_committed(repo)?;
        let head = repo.git(&["rev-parse", "HEAD"])?;
        let mut commit = None;
        for step in join_steps(head == pushed) {
            match step {
                JoinStep::PushBranch => {
                    let to = format!("{head}:refs/heads/{}", pr.head_branch);
                    repo.git(&["push", "--quiet", REMOTE, &to])?;
                    pushed = head.clone();
                }
                JoinStep::Merge => {
                    let train = Train::read(repo, config)?;
                    if repo.within(&head, &train.base)? {
                        let detail =
                            format!("this PR's head is already on {} — stop", config.base_branch);
                        return Ok(stop("merged", json!({"head": head, "detail": detail})));
                    }
                    let joins = train.joins(repo)?;
                    let current = [pr.at_head(&head)];
                    if let Some(on) = on_train(&joins, &current).first() {
                        return Ok(stop("joined", joined(&head, &on.commit, true)));
                    }
                    let message = join_message(&pr.title, pr.number, issue);
                    commit = merged_onto(repo, &train.tip, &head, &message, now)?;
                    if commit.is_none() {
                        let files = merge_in(repo, &train.tip, "the landing train")?;
                        if !files.is_empty() {
                            let detail = format!(
                                "this PR conflicts with what is on the landing train: the train's \
                                 tip was merged into {} and that merge is in progress here — \
                                 {RESOLVE}",
                                pr.head_branch
                            );
                            return Ok(stop("conflict", conflict(files, detail)));
                        }
                        // the train moved between the two merges: join what is there now
                        break;
                    }
                }
                JoinStep::PushLine => {
                    let commit = commit.as_deref().ok_or_else(|| anyhow!("no join commit"))?;
                    let to = format!("{commit}:{line}");
                    // A plain push: the remote takes it only as a fast-forward
                    // of the tip it has. That refusal is the train's one lock.
                    if repo.try_git(&["push", "--quiet", REMOTE, &to])?.ok {
                        return Ok(stop("joined", joined(&head, commit, false)));
                    }
                }
            }
        }
    }
    bail!(
        "the landing train moved {PUSH_TRIES} times while PR #{} was joining it; nothing was \
         changed — run this again",
        pr.number
    )
}

fn joined(head: &str, commit: &str, again: bool) -> Value {
    json!({"head": head, "commit": commit, "again": again,
        "detail": "this PR is on the landing train — stop; the train's worker gates and lands it"})
}

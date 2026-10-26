//! One PR's merge, gate, record and fast-forward landing on the base branch.
use crate::acting::Acting;
use crate::exclusive::Permit;
use crate::gh::{GitHub, Tracker};
use crate::git::{REMOTE, Repo};
use crate::lock::standing_locks;
use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Tree};
use afkconvoy_core::landing::Of;
use afkconvoy_core::lock::{keeps_off, named};
use afkconvoy_core::plan::{PullRequest, closing_pr};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// Fetches the base branch as it stands now, including its objects.
pub fn base(repo: &Repo, config: &Config) -> Result<String> {
    repo.fetched(&format!("refs/heads/{}", config.base_branch))?
        .ok_or_else(|| anyhow!("the remote has no branch {:?}", config.base_branch))
}

/// The base branch's own gate declaration certifies its tip.
pub(crate) fn tip_config(repo: &Repo, tip: &str) -> Result<Config> {
    crate::config::parse(&repo.git_raw(&["show", &format!("{tip}:{}", crate::config::FILE)])?)
}

/// An unrecorded base tip is gated alone, with no worker's changes present.
pub fn certify(repo: &Repo, config: &Config, tip: &str, host: &str, now: u64) -> Result<bool> {
    let command = GateCommand::new(&config.gate_command);
    let _permit = Permit::take(
        repo,
        "gate the base branch's tip",
        crate::config::heavy_gate(command.text()),
        true,
    )?;
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
    if crate::gate::recorded(repo, &tree, &command, now)?.is_some() {
        return Ok(true);
    }
    let path = std::env::temp_dir().join(format!("afkconvoy-base-tip-{}", crate::unique()));
    std::fs::create_dir_all(path.parent().unwrap())?;
    repo.git(&["worktree", "add", "--quiet", "--detach", &path.to_string_lossy(), tip])?;
    let checkout = Repo::new(path.clone());
    let run = crate::gate::run_and_record(
        &checkout,
        &command,
        host,
        now,
        Duration::from_secs(config.gate_seconds()),
    );
    let removed = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
    let run = run?;
    removed?;
    if run.ran.green() && (run.green.is_none() || run.not_recorded.is_some()) {
        bail!(
            "the base tip gated green without a record: {}; run this again",
            run.not_recorded
                .unwrap_or_else(|| format!("uncommitted files: {}", run.strays.join(", ")))
        );
    }
    if !run.ran.green() {
        let red = crate::repair::red_ref(repo, &config.base_branch);
        repo.git(&["push", "--quiet", "--force", REMOTE, &format!("{tip}:{red}")])?;
        Acting::new("gate").in_checkout(Some(repo.clone())).say(
            crate::clock(),
            format!("base tip {tip} is red; worker turns wait for its repair"),
        );
    }
    Ok(run.green.is_some() && run.not_recorded.is_none())
}

/// Acquires one repo's turn on its current certified base tip. A repair
/// issue carrying the fleet lock alone may take a turn on a red base tip.
#[derive(Clone, Copy)]
enum OnRedBase {
    Wait,
    Return,
}

fn take(
    repo: &Repo,
    config: &Config,
    issue: Option<u64>,
    host: &str,
    now: u64,
    on_red: OnRedBase,
) -> Result<Option<String>> {
    let mut tried = None;
    loop {
        let gh = GitHub::of(repo)?;
        let locks = standing_locks(&gh, config)?;
        if !locks.is_empty() && issue.is_none_or(|n| keeps_off(&locks, n)) {
            bail!(
                "a fleet lock stands ({}): no turn starts; run this again when told to carry on",
                named(&locks)
            );
        }
        let tip = base(repo, config)?;
        let declared = tip_config(repo, &tip)?;
        let command = GateCommand::new(&declared.gate_command);
        let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
        let repairing = match issue.filter(|n| locks.contains(n)) {
            Some(n) => crate::repair::is_for(&gh, n, &tip)?,
            None => false,
        };
        if !repairing && crate::gate::recorded(repo, &tree, &command, now)?.is_none() {
            if tried.as_deref() != Some(tip.as_str()) {
                let green = certify(repo, &declared, &tip, host, now)?;
                tried = Some(tip);
                if green {
                    continue;
                }
                let _ = crate::wake::wake(repo, "red base tip");
            }
            if matches!(on_red, OnRedBase::Return) {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        return Ok(Some(tip));
    }
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
pub(crate) fn merged_onto(
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

const RESOLVE: &str =
    "resolve every file, `git add` it, COMMIT the merge, and ask for a turn again";

pub fn locked(mut answer: Value, locks: &[u64]) -> Value {
    answer["outcome"] = json!("locked");
    answer["locks"] = json!(locks);
    answer["detail"] = json!(format!(
        "a fleet lock stands ({}): no other issue lands; run this again when told to carry on",
        named(locks)
    ));
    answer
}

pub fn land(
    repo: &Repo,
    config: &Config,
    gh: &GitHub,
    issue: u64,
    host: &str,
    now: u64,
) -> Result<Value> {
    let prs = gh.open_prs()?;
    let Some(pr) = closing_pr(&prs, issue) else {
        bail!("no open PR closes issue #{issue} — if it has already merged, stop");
    };
    join(
        repo,
        config,
        &gh.tracker,
        pr,
        &Of::Issue(issue),
        json!({"issue": issue, "pr": pr.number}),
        (host, now),
    )
}

/// Lands the PR in one uninterrupted turn. A rejected fast-forward ends the
/// turn; another request reads and gates the new base tip from the start.
pub fn join(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    pr: &PullRequest,
    of: &Of,
    answer: Value,
    (host, now): (&str, u64),
) -> Result<Value> {
    run(repo, config, tracker, pr, of, answer, (host, now, OnRedBase::Wait))
}

/// A reconciliation pass must remain able to dispatch a red-base repair.
/// It leaves the PR pending instead of waiting inside that same pass.
pub fn try_join(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    pr: &PullRequest,
    of: &Of,
    answer: Value,
    (host, now): (&str, u64),
) -> Result<Value> {
    run(repo, config, tracker, pr, of, answer, (host, now, OnRedBase::Return))
}

fn run(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    pr: &PullRequest,
    of: &Of,
    mut answer: Value,
    (host, now, on_red): (&str, u64, OnRedBase),
) -> Result<Value> {
    let gh = GitHub::of(repo)?;
    let issue = match of {
        Of::Issue(n) => Some(*n),
        _ => None,
    };
    let locks = standing_locks(&gh, config)?;
    if issue.map_or(!locks.is_empty(), |n| keeps_off(&locks, n)) {
        return Ok(locked(answer, &locks));
    }
    let files = unmerged(repo)?;
    if !files.is_empty() {
        answer["outcome"] = json!("conflict");
        answer["status"] = json!("conflict");
        answer["recorded"] = json!(false);
        answer["files"] = json!(files);
        answer["worktree"] = json!(repo.dir);
        answer["detail"] = json!(format!("a merge remains in progress — {RESOLVE}"));
        return Ok(answer);
    }
    own_head(repo, &pr.head_branch)?;
    require_committed(repo)?;
    let here = repo.git(&["rev-parse", "HEAD"])?;
    let current = base(repo, config)?;
    if repo.within(&here, &current)? {
        if let Some(n) = issue
            && !gh.issue(n)?.is_closed()
        {
            gh.close_issue(n)?;
        }
        answer["outcome"] = json!("merged");
        answer["commit"] = json!(current);
        return Ok(answer);
    }
    let Some(_tip) = take(repo, config, issue, host, now, on_red)? else {
        answer["outcome"] = json!("base_red");
        return Ok(answer);
    };
    let head = repo.git(&["rev-parse", "HEAD"])?;
    repo.git(&["push", "--quiet", REMOTE, &format!("{head}:refs/heads/{}", pr.head_branch)])?;
    crate::candidates::run(
        crate::candidates::Request { repo, config, tracker, pr, of, host, now },
        answer,
    )
}

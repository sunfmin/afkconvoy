//! One PR's merge, gate, record and fast-forward landing on the base branch.
use crate::acting::Acting;
use crate::exclusive::Permit;
use crate::gh::{GitHub, Tracker};
use crate::git::Repo;
use crate::lock::standing_locks;
use crate::remote::{Fresh, Push};
use afkconvoy_core::account::End;
use afkconvoy_core::config::Config;
use afkconvoy_core::ending::Ending;
use afkconvoy_core::gate::{GateCommand, Tree};
use afkconvoy_core::landing::Of;
use afkconvoy_core::lock::{keeps_off, named};
use afkconvoy_core::plan::{PullRequest, closing_pr};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// Fetches the base branch as it stands now, including its objects.
pub fn base(repo: &Repo, config: &Config) -> Result<String> {
    repo.remote()
        .fetched(&format!("refs/heads/{}", config.base_branch), Fresh::Process)?
        .ok_or_else(|| anyhow!("the remote has no branch {:?}", config.base_branch))
}

/// The base branch's own gate declaration certifies its tip.
pub(crate) fn tip_config(repo: &Repo, tip: &str) -> Result<Config> {
    crate::config::parse(&repo.git_raw(&["show", &format!("{tip}:{}", crate::config::FILE)])?)
}

/// What gating a base tip alone found.
pub struct Certified {
    /// The tip gated green and its run was recorded.
    pub green: bool,
    /// Why the tip's own worktree could not be removed. Its verdict stands
    /// regardless (sunfmin/control#610).
    pub unremoved: Option<String>,
}

/// An unrecorded base tip is gated alone, with no worker's changes present.
pub fn certify(repo: &Repo, config: &Config, tip: &str, host: &str, now: u64) -> Result<Certified> {
    let command = GateCommand::new(&config.gate_command);
    let _permit = Permit::take(
        repo,
        "gate the base branch's tip",
        crate::config::heavy_gate(command.text()),
        true,
    )?;
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
    if crate::gate::recorded(repo, &tree, &command, now)?.is_some() {
        return Ok(Certified { green: true, unremoved: None });
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
        None,
        None,
    );
    let unremoved = repo.remove_worktree(&path).err().map(|error| format!("{error:#}"));
    if let Some(error) = &unremoved {
        eprintln!("warning: {error}");
    }
    let run = run?;
    if run.ran.green() && (run.green.is_none() || run.not_recorded.is_some()) {
        bail!(
            "the base tip gated green without a record: {}; run this again",
            run.not_recorded
                .unwrap_or_else(|| format!("uncommitted files: {}", run.strays.join(", ")))
        );
    }
    if !run.ran.green() {
        let red = crate::repair::red_ref(repo, &config.base_branch);
        repo.remote().push(&Push::set(tip, &red).force())?;
        Acting::new("gate").in_checkout(Some(repo.clone())).say(
            crate::clock(),
            format!("base tip {tip} is red; worker turns wait for its repair"),
        );
    }
    Ok(Certified { green: run.green.is_some() && run.not_recorded.is_none(), unremoved })
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
    unremoved: &mut Vec<String>,
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
        let mut declared = tip_config(repo, &tip)?;
        declared.base_branch = config.base_branch.clone();
        let command = GateCommand::new(&declared.gate_command);
        let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
        let repairing = match issue.filter(|n| locks.contains(n)) {
            Some(n) => crate::repair::is_for(repo, &gh, n, &tip, &config.base_branch)?,
            None => false,
        };
        if !repairing && crate::gate::recorded(repo, &tree, &command, now)?.is_none() {
            if tried.as_deref() != Some(tip.as_str()) {
                let certified = certify(repo, &declared, &tip, host, now)?;
                unremoved.extend(certified.unremoved);
                tried = Some(tip);
                if certified.green {
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

/// The tree of `head` merged onto `base`, or `None` when the two conflict.
/// Made in the object store alone: no worktree is touched and no branch moves.
fn merged_tree(repo: &Repo, base: &str, head: &str) -> Result<Option<String>> {
    let merged = repo.try_git(&["merge-tree", "--write-tree", base, head])?;
    match merged.stdout.split_whitespace().next() {
        Some(tree) if merged.ok => Ok(Some(tree.to_string())),
        // git exits 1 for a merge that conflicts, and names the tree it got to
        Some(_) => Ok(None),
        None => bail!("git merge-tree of {head} onto {base} failed: {}", merged.stderr.trim()),
    }
}

/// Commits `tree` with `parents` and `message` at `now` → its sha.
fn commit_at(repo: &Repo, tree: &str, parents: &[&str], message: &str, now: u64) -> Result<String> {
    let at = format!("@{now} +0000");
    let dated = [("GIT_AUTHOR_DATE", at.as_str()), ("GIT_COMMITTER_DATE", at.as_str())];
    let mut args = vec!["commit-tree".to_string(), tree.to_string()];
    for parent in parents {
        args.push("-p".to_string());
        args.push((*parent).to_string());
    }
    args.push("-m".to_string());
    args.push(message.to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let made = repo.try_git_with(&refs, &dated)?;
    if !made.ok {
        bail!("could not commit {tree}: {}", made.stderr.trim());
    }
    Ok(made.stdout)
}

/// `head` merged onto `base` as ONE merge commit whose second parent is `head`
/// → its sha, `None` when the two conflict. An issue turn and a watch land
/// this commit. A head that changes nothing against `base` still gets its
/// commit — which is what says the PR joined.
pub(crate) fn merged_onto(
    repo: &Repo,
    base: &str,
    head: &str,
    message: &str,
    now: u64,
) -> Result<Option<String>> {
    let Some(tree) = merged_tree(repo, base, head)? else { return Ok(None) };
    Ok(Some(commit_at(repo, &tree, &[base, head], message, now)?))
}

/// `head`'s changes on `base` as one commit whose only parent is `base` → its
/// sha, `None` when the two conflict. A cut lands this commit (ADR-0028). The
/// tree is the merge, so a base that moved keeps the landings already on it
/// and the diff against that parent is what `head` adds. `head` itself is not
/// a parent, and stays off the base branch.
pub(crate) fn cut_onto(
    repo: &Repo,
    base: &str,
    head: &str,
    message: &str,
    now: u64,
) -> Result<Option<String>> {
    let Some(tree) = merged_tree(repo, base, head)? else { return Ok(None) };
    Ok(Some(commit_at(repo, &tree, &[base], message, now)?))
}

/// The head of a PR's `branch` on the remote, with a worktree that is only
/// BEHIND it caught up to it — a worktree recreated before the last push. One
/// that is ahead (a resolution, a fix: the join pushes them) or in the middle
/// of a merge is left as it is.
fn own_head(repo: &Repo, branch: &str) -> Result<String> {
    let tip = repo
        .remote()
        .fetched(&format!("refs/heads/{branch}"), Fresh::Process)?
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

/// The help of `land`: each word a turn can answer, as its ending says it.
pub fn endings_help() -> String {
    Ending::listed(|word, told| format!("- {word}: {told}."))
}

pub fn locked(mut answer: Value, locks: &[u64]) -> Value {
    answer["outcome"] = json!(Ending::Locked);
    answer["locks"] = json!(locks);
    answer["detail"] = json!(format!(
        "a fleet lock stands ({}): no other issue lands; the fleet asks for this turn again once no lock stands",
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
    let looked = crate::target::once(repo, config, &gh.tracker, issue)?;
    let mut answer = crate::turn_answer::noted(repo, Some(issue), now, || {
        land_task(repo, gh, issue, host, now, looked)
    })?;
    crate::delivery::after_task(repo, issue, &mut answer);
    Ok(answer)
}

fn land_task(
    repo: &Repo,
    gh: &GitHub,
    issue: u64,
    host: &str,
    now: u64,
    looked: crate::target::Seen,
) -> Result<Value> {
    let mut routed = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    // The claim, target and base were checked before the turn's receipt.
    // Finding no PR does not read any of those again.
    gh.note_without_probe()?;
    let prs = gh.open_prs()?;
    routed.base_branch = looked.branch;
    let config = &routed;
    let Some(pr) = closing_pr(&prs, issue) else {
        let tip = match (&looked.tip, looked.asked) {
            (Some(sha), _) => {
                repo.bring(sha, &looked.tip_ref)?;
                sha.clone()
            }
            (None, true) => bail!("the remote has no branch {:?}", config.base_branch),
            (None, false) => base(repo, config)?,
        };
        if let Some(landed) = crate::landing::joins_in(repo, &gh.tracker, &[&tip])?
            .iter()
            .rev()
            .find(|pr| pr.issue() == Some(issue))
        {
            if !gh.issue(issue)?.is_closed() {
                gh.close_issue(issue)?;
            }
            crate::account::leave(repo, gh, issue, End::Landed, now)?;
            return Ok(
                json!({"issue": issue, "pr": landed.pr, "outcome": Ending::Merged, "commit": landed.commit, "detail": "recovered landing from base history; issue settled"}),
            );
        }
        bail!("no open PR closes issue #{issue} — if it has already merged, stop");
    };
    if let Some(id) = crate::target::delivery(repo, issue)? {
        let marker = format!("<!--convoy:task-delivery id={id}-->");
        if !gh.pr_comments(pr.number)?.iter().any(|c| c.body.contains(&marker)) {
            gh.comment_on_pr(
                pr.number,
                &format!("{marker}\nTask: {}\nDelivery: {id}", gh.tracker.issue(issue)),
            )?;
        }
    }
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
    let mut routed;
    let config = if let Some(issue) = issue {
        routed = crate::config::load(&repo.dir.join(crate::config::FILE))?;
        routed.base_branch = crate::target::pin(repo, config, issue, None)?;
        &routed
    } else {
        config
    };
    ensure_base(pr, &config.base_branch)?;
    answer["target_branch"] = json!(config.base_branch);
    let files = unmerged(repo)?;
    if !files.is_empty() {
        answer["outcome"] = json!(Ending::Conflict);
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
        if let Some(n) = issue {
            if !gh.issue(n)?.is_closed() {
                gh.close_issue(n)?;
            }
            crate::account::leave(repo, &gh, n, End::Landed, now)?;
        }
        answer["outcome"] = json!(Ending::Merged);
        answer["commit"] = json!(current);
        return Ok(answer);
    }
    let mut unremoved = vec![];
    let taken = take(repo, config, issue, host, now, on_red, &mut unremoved)?;
    if !unremoved.is_empty() {
        answer["unremoved"] = json!(unremoved);
    }
    let Some(_tip) = taken else {
        answer["outcome"] = json!(Ending::BaseRed);
        return Ok(answer);
    };
    let head = repo.git(&["rev-parse", "HEAD"])?;
    repo.remote().push(&Push::set(&head, &format!("refs/heads/{}", pr.head_branch)))?;
    crate::candidates::run(
        crate::candidates::Request { repo, config, tracker, pr, of, host, now },
        answer,
    )
}

pub(crate) fn ensure_base(pr: &PullRequest, target: &str) -> Result<()> {
    if pr.base_branch != target {
        let fix = afkconvoy_core::line::Line::new()
            .fixed("gh pr edit ")
            .number(pr.number)
            .fixed(" --base ")
            .word(target)
            .own();
        bail!(
            "PR #{} has base {:?}, but its pinned target is {target:?}; restore it with {fix}",
            pr.number,
            pr.base_branch
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The words the turn's own code sets as `outcome`: each through an
    /// ending, never as a bare string.
    fn returned() -> BTreeSet<&'static str> {
        let code = [include_str!("turn.rs"), include_str!("candidates.rs")];
        let mut words = BTreeSet::new();
        for text in code {
            let code = text.split("#[cfg(test)]").next().unwrap();
            for bare in ["[\"outcome\"] = json!(\"", "\"outcome\": \""] {
                assert!(!code.contains(bare), "an outcome set as a bare word: {bare}");
            }
            for e in Ending::ALL {
                if code.contains(&format!("Ending::{e:?}")) {
                    words.insert(e.word());
                }
            }
        }
        words
    }

    /// The land step teaches each word the turn returns, and no other.
    #[test]
    fn the_land_step_teaches_exactly_the_words_a_turn_returns() {
        let step = afkconvoy_core::prompt::endings();
        let taught: BTreeSet<&str> =
            step.lines().filter_map(|line| line.split('"').nth(1)).collect();
        assert_eq!(taught, returned(), "{step}");
        assert_eq!(taught.len(), Ending::ALL.len());
    }

    /// The help of `land` states every word the land step teaches, with the
    /// same text, and no meaning of its own.
    #[test]
    fn the_help_of_land_says_each_word_as_the_land_step_does() {
        let help = endings_help();
        for e in Ending::ALL {
            assert!(help.contains(&format!("- {}: {}.", e.word(), e.told())), "{help}");
        }
        assert_eq!(help.lines().count(), Ending::ALL.len());
    }
}

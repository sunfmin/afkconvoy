//! The landing train on the remote, and a worker — or a watch — joining
//! its PR to it.

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Tree};
use afkconvoy_core::lock::{keeps_off, named};
use afkconvoy_core::plan::{PullRequest, closing_pr};
use afkconvoy_core::train::{
    JoinStep, Joined, Line, Logged, Of, PUSH_TRIES, join_message, join_steps, joins_onto, on_train,
    read_joins, train_refs,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::acting::Acting;
use crate::exclusive::Permit;
use crate::gh::{GitHub, Tracker};
use crate::git::{Listing, REMOTE, Repo};
use crate::lock::standing_locks;

/// A tip's green evidence is of the command that tip declares, not a
/// joining branch's replacement for it (which may not exist on the tip).
fn tip_config(repo: &Repo, tip: &str) -> Result<Config> {
    let text = repo.git_raw(&["show", &format!("{tip}:{}", crate::config::FILE)])?;
    crate::config::parse(&text)
}

/// A missing tip record is established in the train lane, never on a
/// worker's changed tree. Keep the checkout separate even when the train
/// is empty: the base branch needs evidence too.
fn certify_tip(repo: &Repo, config: &Config, tip: &str, now: u64) -> Result<bool> {
    let command = GateCommand::new(&config.gate_command);
    let _lane = Permit::take(repo, "gate the train's tip", true, true, false)?;
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
    if crate::gate::recorded(repo, &tree, &command, now)?.is_some() {
        return Ok(true);
    }
    let path = std::env::temp_dir().join(format!("afkconvoy-tip-{}", crate::unique()));
    repo.git(&["worktree", "add", "--quiet", "--detach", &path.to_string_lossy(), tip])?;
    let checkout = Repo::new(path.clone());
    let run = crate::gate::run_and_record(
        &checkout,
        &command,
        &crate::this_host(),
        now,
        std::time::Duration::from_secs(config.gate_seconds()),
    );
    // Remove our own detached checkout on red and on errors as on green.
    let removed = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
    let run = run?;
    removed?;
    if run.ran.green() && (run.green.is_none() || run.not_recorded.is_some()) {
        let why = run.not_recorded.unwrap_or_else(|| {
            format!("the gate left uncommitted files: {}", run.strays.join(", "))
        });
        bail!(
            "the train's tip {tip} gated green, but has no record: {why}; run this again — no worker gate ran and nothing joined"
        );
    }
    Ok(run.green.is_some() && run.not_recorded.is_none())
}

/// Wait outside the worker lane while the train has no green tip. A train
/// lane run can establish a missing record; a red tip is the train worker's
/// to repair. Re-read after acquiring the worker lane, since it can move
/// while we wait there. A lock issue's turn waits for no green tip: the
/// train worker that raised the lock on a red base branch waits for that
/// issue's fix to join first, and the turn's own gate of the merged tree
/// decides whether it does.
fn turn(repo: &Repo, config: &Config, issue: Option<u64>, now: u64) -> Result<(Permit, Train)> {
    let mut tried = None;
    loop {
        let gh = GitHub::of(repo)?;
        let locks = standing_locks(&gh, config)?;
        if !locks.is_empty() && issue.is_none_or(|issue| keeps_off(&locks, issue)) {
            bail!(
                "a fleet lock stands ({}): this command takes no turn; run this again when told to carry on",
                named(&locks)
            );
        }
        // Past the check above, a standing lock means this is its issue.
        let repairs = !locks.is_empty();
        let train = Train::read(repo, config)?;
        let tip_config = tip_config(repo, &train.tip)?;
        let command = GateCommand::new(&tip_config.gate_command);
        let tree = Tree::named(&repo.git(&["rev-parse", &format!("{}^{{tree}}", train.tip)])?);
        if !repairs && crate::gate::recorded(repo, &tree, &command, now)?.is_none() {
            if tried.as_deref() != Some(train.tip.as_str()) {
                let green = certify_tip(repo, &tip_config, &train.tip, now)?;
                tried = Some(train.tip.clone());
                if green {
                    continue;
                }
                let acting = Acting::new("gate").on_own_issue().in_checkout(Some(repo.clone()));
                let line = format!(
                    "waits for a green recorded gate run of train tip {}; no turn starts",
                    train.tip
                );
                eprintln!("[afkconvoy] {line}");
                acting.say(crate::clock(), line);
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
            continue;
        }
        let heavy = crate::config::heavy_gate(&config.gate_command)
            || crate::config::heavy_gate(&tip_config.gate_command);
        let permit = Permit::take(repo, "turn: merge, gate, join", false, heavy, true)?;
        let current = Train::read(repo, config)?;
        let current_tree =
            Tree::named(&repo.git(&["rev-parse", &format!("{}^{{tree}}", current.tip)])?);
        let current_config = self::tip_config(repo, &current.tip)?;
        if !heavy && crate::config::heavy_gate(&current_config.gate_command) {
            drop(permit);
            continue;
        }
        let current_command = GateCommand::new(&current_config.gate_command);
        if !repairs && crate::gate::recorded(repo, &current_tree, &current_command, now)?.is_none()
        {
            drop(permit);
            continue;
        }
        return Ok((permit, current));
    }
}

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
    /// oldest first, as the fleet whose tracker is `tracker` reads them.
    pub fn joins(&self, repo: &Repo, tracker: &Tracker) -> Result<Vec<Joined>> {
        if self.tip == self.base {
            return Ok(vec![]);
        }
        joins_in(repo, tracker, &[&format!("{}..{}", self.base, self.tip)])
    }
}

/// The joinings among the commits `revs` selects, oldest first, as the fleet
/// whose tracker is `tracker` reads them. Whatever narrows the walk
/// (`--grep`) never decides what a commit is.
pub fn joins_in(repo: &Repo, tracker: &Tracker, revs: &[&str]) -> Result<Vec<Joined>> {
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
    Ok(read_joins(&commits, tracker))
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
/// train, from the worktree it is called in → as `join` says. With a fleet
/// lock standing, the PR of an issue that is not one joins nothing →
/// `locked`, and nothing is pushed.
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
        bail!(
            "no open PR closes issue #{issue} — there is nothing to land (if it has already \
             merged, stop)"
        );
    };
    let answer = json!({"issue": issue, "pr": pr.number});
    let locks = standing_locks(gh, config)?;
    if keeps_off(&locks, issue) {
        return Ok(locked(answer, &locks));
    }
    join(repo, config, &gh.tracker, pr, &Of::Issue(issue), answer, (host, now))
}

/// `answer`, with the `locked` outcome the standing `locks` give it.
pub fn locked(mut answer: Value, locks: &[u64]) -> Value {
    let detail = format!(
        "a fleet lock stands ({}): no PR of another issue joins the landing train until it has \
         ended — stop, and run this again when you are told to carry on",
        named(locks)
    );
    answer["outcome"] = json!("locked");
    answer["locks"] = json!(locks);
    answer["detail"] = json!(detail);
    answer
}

/// Puts `pr`, whose branch is checked out in the worktree `repo`, on the
/// landing train, its join commit naming whose it is (`of`), an issue as the
/// repo names one of `tracker`'s → `answer` with
/// the outcome: `joined`, `conflict`, or `merged` when the base branch
/// already holds the PR's head. No gate runs here: the train's own worker
/// gates the train and lands it.
///
/// The push of the train's tip is the only lock: refused — another PR joined
/// first — the join is made again on the new tip. Nothing on the train is
/// rewritten, so a PR that conflicts with it resolves that once.
pub fn join(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    pr: &PullRequest,
    of: &Of,
    answer: Value,
    (host, now): (&str, u64),
) -> Result<Value> {
    own_head(repo, &pr.head_branch)?;
    let head = repo.git(&["rev-parse", "HEAD"])?;
    let train = Train::read(repo, config)?;
    if repo.within(&head, &train.base)?
        || !on_train(&train.joins(repo, tracker)?, &[pr.at_head(&head)]).is_empty()
    {
        return join_committed(repo, config, tracker, pr, of, answer, now);
    }
    let issue = match of {
        Of::Issue(issue) => Some(*issue),
        _ => None,
    };
    let acting = Acting::new("gate").on_issue(issue).in_checkout(Some(repo.clone()));
    loop {
        let (_permit, train) = turn(repo, config, issue, now)?;
        let locks = standing_locks(&GitHub::of(repo)?, config)?;
        if issue.map_or(!locks.is_empty(), |issue| keeps_off(&locks, issue)) {
            return Ok(locked(answer, &locks));
        }
        let files = merge_in(repo, &train.tip, "the train's green tip")?;
        if !files.is_empty() {
            let mut answer = answer;
            answer["status"] = json!("conflict");
            answer["outcome"] = json!("conflict");
            answer["recorded"] = json!(false);
            answer["files"] = json!(files);
            answer["worktree"] = json!(repo.dir);
            answer["detail"] =
                json!(format!("merging the train's green tip conflicted; no gate ran — {RESOLVE}"));
            acting.say(
                crate::clock(),
                format!("turn merged {}: conflict; nothing joined", train.tip),
            );
            return Ok(answer);
        }
        acting.say(crate::clock(), format!("turn merged {}", train.tip));
        let merged_config = crate::config::load(&crate::config::file_of(repo))?;
        let mut gated = crate::gate::gate(repo, &merged_config, false, host, now)?;
        gated["merged"] = json!(train.tip);
        acting.say(
            crate::clock(),
            format!("turn gate {}", gated["status"].as_str().unwrap_or("unknown")),
        );
        if gated["status"] != "green" || gated["recorded"] != true {
            for (key, value) in answer.as_object().into_iter().flatten() {
                gated[key] = value.clone();
            }
            gated["outcome"] = json!("gate_red");
            return Ok(gated);
        }
        let mut joined = join_committed(repo, config, tracker, pr, of, answer.clone(), now)?;
        if joined["outcome"] == "tip_moved" {
            continue;
        }
        for (key, value) in gated.as_object().into_iter().flatten() {
            if key != "detail" {
                joined[key] = value.clone();
            }
        }
        acting.say(
            crate::clock(),
            format!("turn join {}", joined["outcome"].as_str().unwrap_or("unknown")),
        );
        return Ok(joined);
    }
}

fn join_committed(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    pr: &PullRequest,
    of: &Of,
    mut answer: Value,
    now: u64,
) -> Result<Value> {
    let line = train_refs(repo.layout(), &config.base_branch).line;
    let mut stop = |outcome: &str, more: Value| {
        answer["outcome"] = json!(outcome);
        for (key, value) in more.as_object().into_iter().flatten() {
            answer[key] = value.clone();
        }
        answer.clone()
    };
    let conflict = |files: Vec<String>, detail: String| json!({"files": files, "worktree": repo.dir, "detail": detail});
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
                    let joins = train.joins(repo, tracker)?;
                    let current = [pr.at_head(&head)];
                    if let Some(on) = on_train(&joins, &current).first() {
                        return Ok(stop("joined", joined(&head, &on.commit, true)));
                    }
                    let message = join_message(&pr.title, pr.number, of, tracker);
                    commit = merged_onto(repo, &train.tip, &head, &message, now)?;
                    if let Some(commit) = &commit {
                        let joined_tree =
                            repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])?;
                        let tested_tree = repo.git(&["rev-parse", "HEAD^{tree}"])?;
                        if joined_tree != tested_tree {
                            return Ok(stop(
                                "tip_moved",
                                json!({"detail": "the train moved; take another turn on its new green tip"}),
                            ));
                        }
                    }
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

/// Joins `pr`, at its branch's head on the remote, onto the train as it
/// stands, from no worktree: a watch's PR taken off a train that was
/// given up, whose watch is gone and joins nothing again itself →
/// whether it is on the train, or on the base branch, now; false where it
/// conflicts with the train, and nothing was pushed.
pub fn join_again(
    repo: &Repo,
    config: &Config,
    tracker: &Tracker,
    pr: &PullRequest,
    of: &Of,
    now: u64,
) -> Result<bool> {
    let branch = format!("refs/heads/{}", pr.head_branch);
    let head =
        repo.fetched(&branch)?.ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
    let path = std::env::temp_dir().join(format!("afkconvoy-rejoin-{}", crate::unique()));
    repo.git(&["worktree", "add", "--quiet", "--detach", &path.to_string_lossy(), &head])?;
    let checkout = Repo::new(path.clone());
    let answer = join(&checkout, config, tracker, pr, of, json!({}), (&crate::this_host(), now));
    let removed = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
    let answer = answer?;
    removed?;
    Ok(matches!(answer["outcome"].as_str(), Some("joined" | "merged")))
}

fn joined(head: &str, commit: &str, again: bool) -> Value {
    json!({"head": head, "commit": commit, "again": again,
        "detail": "this PR is on the landing train — stop; the train's worker gates and lands it"})
}

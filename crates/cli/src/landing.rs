//! The train worker's run: gate the landing train as it stands and land it on
//! the base branch.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Proven, Tree, red_comment};
use afkconvoy_core::train::{PUSH_TRIES, TrainOutcome, finished, keeps_its_commits, train_refs};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::gate::{Gated, gated};
use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::train::{Train, merge_in, require_committed, unmerged};

/// How long a run of `land --train` is given.
pub struct Limits {
    /// The gate command, before it is killed and red.
    pub gate: Duration,
    /// GitHub, to show the PRs a landing landed as merged.
    pub merged: Duration,
}

/// Pushes `commit` to the base branch as a fast-forward → whether the remote
/// took it. It asks for the proof of the commit's tree, and pushes nothing
/// that proof is not of.
fn push_base(repo: &Repo, branch: &str, commit: &str, proven: &Proven) -> Result<bool> {
    let tree = repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])?;
    if tree != proven.tree() {
        bail!(
            "{commit} does not hold the tree the gate proved ({}); nothing landed",
            proven.tree()
        );
    }
    let to = format!("{commit}:refs/heads/{branch}");
    Ok(repo.try_git(&["push", "--quiet", REMOTE, &to])?.ok)
}

/// Waits for GitHub to show the PRs `numbers` merged — it does so by itself, a
/// moment after a push puts their heads on the base branch → the ones still
/// open when `timeout` ran out.
fn await_merged(gh: &GitHub, numbers: &BTreeSet<u64>, timeout: Duration) -> Result<Vec<u64>> {
    let started = Instant::now();
    loop {
        let open = gh.open_prs()?;
        let still: Vec<u64> =
            numbers.iter().copied().filter(|n| open.iter().any(|pr| pr.number == *n)).collect();
        let left = timeout.saturating_sub(started.elapsed());
        if still.is_empty() || left.is_zero() {
            return Ok(still);
        }
        std::thread::sleep(left.min(Duration::from_secs(2)));
    }
}

/// `afkconvoy land --train`: the train worker gates the train as it stands
/// and lands it, in the train's worktree (the one it is called from).
///
///   take    the train's tip: the worktree starts over on it, or — holding a
///           fix made here while another PR joined — merges it in
///   base    the base branch, merged in when it moved from outside the train:
///           a merge, never a rebase — nothing on a train is rewritten
///   push    what that made, as the train's new tip
///   gate    the gate command on that commit, timed and recorded as a run on
///           `host` — or the run on record for its tree, left as it is
///   land    the commit pushed to the base branch as a fast-forward
///   finish  every PR that commit landed: its issue closed, and its branch
///           deleted once GitHub shows it merged
///
/// The commit pushed to the base branch is the commit gated, and a base branch
/// that moved refuses the push. Which PRs a run landed is read off the join
/// commits between the base branch and that commit.
pub fn land_train(
    repo: &Repo,
    config: &Config,
    gh: &GitHub,
    host: &str,
    now: u64,
    limits: &Limits,
) -> Result<Value> {
    let branch = &config.base_branch;
    let refs = train_refs(repo.layout(), branch);
    let command = GateCommand::new(&config.gate_command);
    let mut answer = json!({"train": refs.line, "base": branch});
    let stop = |answer: &Value, outcome: TrainOutcome, more: Value| {
        let mut answer = answer.clone();
        answer["outcome"] = json!(outcome.word());
        for (key, value) in more.as_object().into_iter().flatten() {
            answer[key] = value.clone();
        }
        answer
    };
    let conflict = |answer: &Value, what: &str, files: Vec<String>| {
        let detail = format!(
            "merging {what} into the train conflicted; the merge is in progress here — resolve \
             every file, `git add` it, COMMIT the merge, and run this again"
        );
        stop(
            answer,
            TrainOutcome::Conflict,
            json!({"files": files, "worktree": repo.dir, "detail": detail}),
        )
    };

    let files = unmerged(repo)?;
    if !files.is_empty() {
        return Ok(conflict(&answer, "it", files));
    }
    require_committed(repo)?;
    let mut taken = None;
    for _ in 0..PUSH_TRIES {
        let train = Train::read(repo, config)?;
        let ahead = train.tip != train.base;
        let here = repo.git(&["rev-parse", "HEAD"])?;
        let own = here != train.tip && !repo.within(&here, &train.tip)?;
        // Commits here are work on this train when they fork from it above the base branch.
        let fork = repo.try_git(&["merge-base", &here, &train.tip])?;
        let on_this_train = fork.ok && !repo.within(&fork.stdout, &train.base)?;
        if keeps_its_commits(ahead, own, on_this_train) {
            let files = merge_in(repo, &train.tip, "the landing train")?;
            if !files.is_empty() {
                return Ok(conflict(&answer, "the train's tip", files));
            }
        } else {
            repo.git(&["reset", "--quiet", "--hard", &train.tip])?;
        }
        if !ahead {
            let detail = "nothing is on the landing train — stop; you are told when a PR joins";
            return Ok(stop(&answer, TrainOutcome::Idle, json!({"detail": detail})));
        }
        if !repo.within(&train.base, "HEAD")? {
            let files = merge_in(repo, &train.base, branch)?;
            if !files.is_empty() {
                return Ok(conflict(&answer, branch, files));
            }
        }
        let head = repo.git(&["rev-parse", "HEAD"])?;
        let to = format!("{head}:{}", refs.line);
        if head == train.tip || repo.try_git(&["push", "--quiet", REMOTE, &to])?.ok {
            taken = Some((train, head));
            break;
        }
    }
    let Some((train, head)) = taken else {
        bail!(
            "the landing train moved {PUSH_TRIES} times under this run; nothing landed — run \
             this again"
        );
    };
    let joins = Train { tip: head.clone(), base: train.base.clone() }.joins(repo)?;
    let prs: BTreeSet<u64> = joins.iter().map(|join| join.pr).collect();
    answer["commit"] = json!(head);
    answer["prs"] = json!(prs);

    let (run, source) = match gated(repo, &command, host, now, limits.gate)? {
        Gated::Green { run, source } => (run, source),
        Gated::Red { ran, excerpt } => {
            // Best effort: a red commit not on record only costs a worker's gate a red run.
            let _ = repo.try_git(&[
                "push",
                "--quiet",
                "--force",
                REMOTE,
                &format!("{head}:{}", refs.red),
            ]);
            let comment = red_comment(&command, ran, &excerpt);
            for pr in gh.open_prs()?.iter().filter(|pr| prs.contains(&pr.number)) {
                gh.comment(pr.number, &comment)?;
            }
            let detail = "the gate is red on the train's tip. Fix the TRAIN: read the failure, \
                          change what makes it green, and COMMIT — one more commit on top, here. \
                          Do not hunt for the PR at fault and drop nothing. Then run this again";
            let gate = json!({"status": "red", "exit_code": ran.exit_code,
                "timed_out": ran.timed_out, "seconds": ran.seconds, "command": command.text(),
                "excerpt": excerpt});
            return Ok(stop(
                &answer,
                TrainOutcome::GateRed,
                json!({"gate": gate, "detail": detail}),
            ));
        }
    };
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{head}^{{tree}}")])?);
    let Some(proven) = Proven::by(&tree, &command, &run, now) else {
        bail!(
            "the green run of the gate does not stand for {head}; nothing landed — run this again"
        );
    };
    answer["gate"] = json!({"status": "green", "source": source, "command": command.text(),
        "at": proven.at()});
    // A run recorded before records kept a time has none to say.
    if let Some(took) = run.took() {
        answer["gate"]["seconds"] = json!(took.seconds());
        answer["gate"]["host"] = json!(took.host());
    }

    // read before the push: GitHub takes a PR off the open list the moment its head lands
    let open = gh.open_prs()?;
    if !push_base(repo, branch, &head, &proven)? {
        if repo.remote_sha(&format!("refs/heads/{branch}"))?.as_deref() == Some(&train.base) {
            bail!(
                "the push of the landing train to {branch} failed although {branch} has not moved"
            );
        }
        let detail = format!(
            "{branch} moved while the gate ran: the fast-forward was refused and nothing landed \
             — run this again; it merges the new tip in and gates that"
        );
        return Ok(stop(&answer, TrainOutcome::TargetMoved, json!({"detail": detail})));
    }

    // On the base branch: landed. The claims and the PRs' worktrees are the tick's.
    let landed = finished(&joins, &open);
    for join in &landed {
        if !gh.issue(join.issue)?.is_closed() {
            gh.close_issue(join.issue)?;
        }
    }
    let numbers: BTreeSet<u64> = landed.iter().map(|join| join.pr).collect();
    let still_open = await_merged(gh, &numbers, limits.merged)?;
    for pr in
        open.iter().filter(|pr| numbers.contains(&pr.number) && !still_open.contains(&pr.number))
    {
        // Soft: a branch already gone is what was wanted.
        let _ = repo.try_git(&[
            "push",
            "--quiet",
            REMOTE,
            "--delete",
            &format!("refs/heads/{}", pr.head_branch),
        ]);
    }
    let more = repo.remote_sha(&refs.line)?.as_deref() != Some(&head);
    let rows: Vec<Value> = landed
        .iter()
        .map(|join| json!({"issue": join.issue, "pr": join.pr, "commit": join.commit}))
        .collect();
    let detail = if more {
        "landed — and the train has grown since: run this again"
    } else {
        "landed — run this again; it says `idle` when the train is empty"
    };
    Ok(stop(
        &answer,
        TrainOutcome::Landed,
        json!({"landed": rows, "unmerged": still_open, "more": more, "detail": detail}),
    ))
}

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
use afkconvoy_core::gate::{GateCommand, Judged, Tree};
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

/// How the base tip's gate run alone came out.
pub enum Certified {
    Green,
    /// Red, and recorded red: worker turns wait for its repair. A run made
    /// by this turn says how it went red (`red_tip`); a tip recorded red
    /// before says nothing more.
    Red(Option<String>),
    /// Red only on TIMEOUTs the gate said it cannot classify on an
    /// overloaded host (`gate::Judged::Unsettled`): it says nothing of the
    /// tip, which is recorded neither green nor red, and is gated alone again
    /// by the next turn. `output` keeps the run's whole log.
    Unsettled {
        unsettled: afkconvoy_core::gate::Unsettled,
        output: std::path::PathBuf,
    },
}

/// An unrecorded base tip is gated alone, with no worker's changes present.
/// The run's whole log is kept in the checkout's git dir, in a file of its
/// own, unless it is green: what a red tip's repair reads.
pub fn certify(
    repo: &Repo,
    config: &Config,
    tip: &str,
    host: &str,
    now: u64,
    unremoved: &mut Vec<String>,
) -> Result<Certified> {
    let command = GateCommand::new(&config.gate_command);
    let _permit = Permit::take(
        repo,
        "gate the base branch's tip",
        crate::config::heavy_gate(command.text()),
        true,
    )?;
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
    if crate::gate::recorded(repo, &tree, &command, now)?.is_some() {
        return Ok(Certified::Green);
    }
    let output =
        repo.common_dir()?.join(format!("convoy-base-tip-gate-{now}-{}.log", crate::unique()));
    let (run, removal) = crate::gate::detached(repo, tip, "base-tip", |checkout| {
        crate::gate::run_and_record(
            checkout,
            &command,
            host,
            now,
            Duration::from_secs(config.gate_seconds()),
            None,
            Some(&output),
        )
    })?;
    // A worktree that cannot be removed does not hide the verdict: why is
    // added to `unremoved`, beside it (sunfmin/control#610).
    if let Some(error) = removal {
        eprintln!("warning: {error:#}");
        unremoved.push(format!("{error:#}"));
    }
    let run = run?;
    let shown = output.display();
    match run.judged {
        Judged::Green => {
            let _ = std::fs::remove_file(&output);
            if run.green.is_none() || run.not_recorded.is_some() {
                bail!(
                    "the base tip gated green without a record: {}; run this again",
                    run.not_recorded
                        .unwrap_or_else(|| format!("uncommitted files: {}", run.strays.join(", ")))
                );
            }
        }
        Judged::Unsettled(unsettled) => {
            Acting::new("gate").in_checkout(Some(repo.clone())).say(
                crate::clock(),
                format!(
                    "base tip {tip} is unsettled: {}; not recorded red, the next turn gates it \
                     again; its output is in {shown}",
                    unsettled.how()
                ),
            );
            return Ok(Certified::Unsettled { unsettled, output });
        }
        Judged::Red(red) => {
            let red_ref = crate::repair::red_ref(repo, &config.base_branch);
            repo.remote().push(&Push::set(tip, &red_ref).force())?;
            let said = red_tip(tip, &red);
            Acting::new("gate")
                .in_checkout(Some(repo.clone()))
                .say(crate::clock(), format!("{said}; its output is in {shown}"));
            return Ok(Certified::Red(Some(said)));
        }
    }
    Ok(Certified::Green)
}

/// What a base tip gated alone red is told by: the one red wording
/// (`Red::said`) and what the fleet does of it.
fn red_tip(tip: &str, red: &afkconvoy_core::gate::Red) -> String {
    format!("{}; worker turns wait for its repair", red.said(&format!("base tip {tip}")))
}

/// Acquires one repo's turn on its current certified base tip. A repair
/// issue carrying the fleet lock alone may take a turn on a red base tip.
#[derive(Clone, Copy)]
enum OnRedBase {
    Wait,
    Return,
}

/// Takes the turn on a tip that is green on record (`Certified::Green`), or
/// answers why there is none.
fn take(
    repo: &Repo,
    config: &Config,
    issue: Option<u64>,
    host: &str,
    now: u64,
    on_red: OnRedBase,
    unremoved: &mut Vec<String>,
) -> Result<Certified> {
    let mut tried = None;
    let mut said = None;
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
                match certify(repo, &declared, &tip, host, now, unremoved)? {
                    Certified::Green => {
                        tried = Some(tip);
                        continue;
                    }
                    // A tip no run settled is not waited on as red: the
                    // turn ends, and the next one gates it alone again.
                    unsettled @ Certified::Unsettled { .. } => return Ok(unsettled),
                    Certified::Red(red) => {
                        said = red;
                        tried = Some(tip);
                        let _ = crate::wake::wake(repo, "red base tip");
                    }
                }
            }
            if matches!(on_red, OnRedBase::Return) {
                return Ok(Certified::Red(said));
            }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        return Ok(Certified::Green);
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
    match taken {
        Certified::Green => {}
        Certified::Red(said) => {
            answer["outcome"] = json!(Ending::BaseRed);
            if let Some(said) = said {
                answer["detail"] = json!(said);
            }
            return Ok(answer);
        }
        Certified::Unsettled { unsettled, output } => {
            answer["outcome"] = json!(Ending::BaseUncertified);
            answer["detail"] = json!(format!(
                "the base tip gated alone {}: it is recorded neither green nor red, and the next \
                 turn gates it alone again; its output is in {}",
                unsettled.how(),
                output.display()
            ));
            return Ok(answer);
        }
    }
    let head = repo.git(&["rev-parse", "HEAD"])?;
    repo.remote().push(&Push::set(&head, &format!("refs/heads/{}", pr.head_branch)))?;
    crate::candidates::run(
        crate::candidates::Request { repo, config, tracker, pr, of, host, now },
        answer,
    )
}

pub(crate) fn ensure_base(pr: &PullRequest, target: &str) -> Result<()> {
    ensure_target(pr.number, &pr.base_branch, target)
}

/// Refuses PR `number` whose base is `base` where its pinned target is
/// `target`.
pub(crate) fn ensure_target(number: u64, base: &str, target: &str) -> Result<()> {
    if base != target {
        let fix = afkconvoy_core::line::Line::new()
            .fixed("gh pr edit ")
            .number(number)
            .fixed(" --base ")
            .word(target)
            .own();
        bail!(
            "PR #{number} has base {base:?}, but its pinned target is {target:?}; restore it with {fix}"
        );
    }
    Ok(())
}

/// What `afkconvoy land --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

After the PR is open and all changes are committed: `afkconvoy land <n>` or `afkconvoy land --issue <n>`. A watch uses `land --watch` for its own PR. Cuts take the same turn automatically.

A bound repo's landing worker can stop with `afkconvoy verdict blocked --issue <n> --stage land --blocked-by <issues> --text-file <file>` when another issue must be fixed first. The fleet parks its claim on open blockers and returns it to Landing when they all close. `verdict escalate --issue <n> --stage land --text-file <file>` hands it to a human, with the PR and branch kept; `afkconvoy rule` returns the issue to Landing after the cause is resolved.

## Who runs it

An issue's worker in its own worktree; in a bound workflow its landing worker after the last stage passes. A watch uses its own linked checkout named {const:WATCH_CHECKOUT}, with an optional numeric suffix. Other watch checkout names are refused.

The issue's claim is checked live first. If another host holds it, the command refuses: `the claim on #N is held by instance X on host H since <ts>: this worker's work was taken; stop`. No candidate, gate record or landing is written. Ownership uses the host's name from its host settings, regardless of --host; a new instance on the same host keeps its workers.

## What it reads and writes

A task uses its pinned target branch (`afkconvoy target --help`), or resolves the configured base when it has no target. Its PR must actually name that base; a wrong base, missing branch or unreadable target refuses without falling back. Each repo and target has its own candidate chain and landed commits. Watches and cuts retain the configured base.

{const:REMOTE_READS}

A turn preserves the PR's original head and atomically assigns a candidate merged onto the certified base or its preceding candidate. Each candidate must gate that exact commit in its own worktree, and its gate run is recorded green against it, unless its tree is already green on record of the same gate on this kind of machine: that run is reused, nothing runs, and the candidate's evidence names it under `reused`; a round's slow gate is reused the same way. A gate-record push that does not end within 30 seconds is killed with its process group: `not_recorded` says so, the candidate is not green, and queued work can take its lane. With free worker lanes, candidates of the same repo validate concurrently; green successors wait for their exact predecessors, then push that commit to the target branch as a fast-forward without another gate. A candidate holds a worker lane only while it builds and runs its gate; the window has no bound of its own, but for barriers and unconfirmed cancellation. A light gate keeps its repo lane. Committed source changes are pushed to the PR first; candidate merges never move that source branch. The merge message names the PR and issue or watch. Landing closes the issue; the tick settles its claim and source worktree. Once the landing is published, a failure to close the issue or write its work account does not turn the answer into an error: it is `merged`, with the failure in `settle_error`, and the tick settles the issue; a close GitHub refuses while it closes the issue itself, as a merged PR that says `Closes` it, is a close.

Where the candidate's commit declares a ```yaml gates block (`afkconvoy config --help`), the candidate runs the fast gate, not `gate_command`, and waits fast-green for a round. A round is as big as the PRs that are ready: a turn that asks to land is on the candidate record as pending while it waits for a worker lane, and a fast-green candidate frees its lane. When no turn is pending and every candidate of the chain is fast-green, the turn of the chain's last candidate, the round's tip, takes a slow lane and runs the slow gate once on its commit, in its worktree; a PR that asks meanwhile waits for the next round. A round waits no longer once four candidates at the chain's head are fast-green: the last of that fast-green head is its tip, and the turns still pending or gating behind it join the next round, so a steady stream of turns never starves a round. Green, the whole gate is put on record for the tip's tree from its fast and slow runs, and one atomic push lands every candidate up to the tip; each of their turns answers `merged` and closes its own issue. Red is not split to find whose change failed: the tip's turn answers `gate_red` with `gate` slow and `round`, the round's PRs and issues, and merges the round's tip into its branch, so its worker fixes the round from those issues and the failing tests and asks again; every other candidate of the round answers `round_red` and stops, its change carried by the tip's PR, and its issue is settled from the merge commits on the target branch once that lands. A merge onto the candidate ahead that conflicts is left in the worker's worktree to resolve, as `speculative_conflict`: the worker's branch then carries that candidate. If a fast-green candidate's owner exits on this host after its gate processes are confirmed stopped, the next turn of the same PR and original head adopts it with its recorded evidence and successors intact; a running gate is never adopted.

Candidate worktrees are kept in `candidates/<repo-key>/<candidate-id>` under the host's cache directory (`afkconvoy help cache`), outside the checkout: a gate's tools discover the candidate's own configuration without finding the checkout's above it. The repo key hashes the canonical common git directory, so all worktrees of one repository share a root and separate clones have separate roots.
A cache configured inside any checkout is refused before reserving a turn or assigning a candidate. Choose a cache outside every checkout (`afkconvoy help cache`) and run `afkconvoy land` again.

Once a candidate has stopped — landed, red, or cancelled with its processes confirmed gone — its worktree is removed. Removal accepts only that repo's candidate root and the earlier `convoy-candidates` root in its common git directory, so a candidate left by an older version is removed too. The candidate chain keeps the gate evidence: the phase, the cause and the gate excerpt, and a recorded gate run stays on the remote. A directory left behind after git dropped the registration is removed too, including when a directory inside it is not writable by its owner. A candidate that is building, validating, green and waiting on its predecessor or for a round, slow-gating, or cancelling, keeps its worktree. A worktree a process still has as its working directory stays until that process is gone, and a later tick removes it. The issue's, watch's or cut's own worktree is left, and the candidate chain is not rewritten.

The remote candidate record holds original head, predecessor, commit, tree, gate declaration, owner, process group, validation and cancellation. Publication rechecks validity, the PR's actual base, head, predecessor landing, issue blockers, fleet locks and the green record, then atomically fast-forwards the exact tested candidate, or a round's tip with every candidate it lands, and its state. No force push or extra merge is used; a round's slow gate is the one gate of more than one candidate. Known candidate advances preserve successors; an outside base move or changed PR head invalidates the affected suffix. Cuts, watches, fleet-lock repairs and base certification drain the candidate window and run as serial barriers.

Failure invalidates the dependent suffix before terminating its running gates and children. Their lanes remain occupied until exit is confirmed. Cancellation does not record a dependent worker's gate as failed or increase its rework count. A later turn rebuilds from preserved original heads and the last landed base, respecting issue blockers. Gate startup waits until its group is recorded; inherited kernel lane leases prevent duplicate capacity after runner or owner death. Interrupted cancellation is completed on the owning host. Another host's processes are never signalled from here, but that host is dead once nothing of it said it was alive within the lease, {const:LEASE_MINUTES} minutes: none of its turns stamped the candidate record, which each does at least every {const:BEAT_EVERY_MINUTES} minutes while it waits, and no fleet instance of it beat on this remote. The next turn on another host takes over what a dead host holds on the chain: its candidates are cancelled `owner_died` with their suffix and no longer hold the chain, unconfirmed, and its pending turns and barriers are dropped. A turn of that host that comes back learns it at its next look at the chain: it stops its own gate and answers `cancelled`, cause `owner_died`; one still waiting for a candidate takes a place again, at the back. A successful base push records its landed prefix atomically, and its merge commit lets recovery settle the issue after a crash.

Publication uses an explicit atomic Git receive-pack transaction so an unchanged source head is verified alongside the base and candidate record. Normal git push omits that unchanged-ref condition. Local remotes use git; SSH origins use their existing SSH login. A GitHub HTTPS origin publishes through GitHub's all-or-none `updateRefs` after a plain `git push` carries the objects: `gh` must be logged in with write access and git's credential helper must give that login (`gh auth setup-git`). No token is read or handled. Other HTTP transports require an SSH origin. Failed authentication or missing atomic capability refuses before any ref update is sent.

A watch finds its branch's PR through REST, and every turn checks fleet locks through REST. Exhausted GraphQL quota alone therefore does not prevent a watch from landing its repair; an unreadable lock listing still refuses the turn.

An unrecorded base tip is gated alone in a separate checkout using its own gate declaration. While it is red, ordinary turns wait outside worker lanes. The fleet opens a dispatchable fleet-lock issue for its repair. That issue's worker alone may take a turn on the red tip, and must still gate its complete merge green before landing. A standing fleet lock allows only its issues to land.

## What it answers

{"outcome", "pr", "commit", "merged", "candidate", "predecessor", "target_branch", "status", "recorded", "detail", …}. issue or watch identifies whose PR it is. merged and predecessor name the assigned predecessor; commit is the exact candidate landed. status and recorded describe the gate, with head, command, exit_code, timed_out, seconds and excerpt. doctor and the panel expose the same candidate lifecycle with validation, predecessor, waiting and cancellation cause.

{const:LAND_ENDINGS}

Errors are {"error": …} with non-zero exit: no open PR, unreadable config or remote, uncommitted tracked changes, an invalid watch checkout or a green base record that could not be written. detail names what to fix.

## What trips you up

- The turn owns its lane through landing. Never run gh pr merge or push the base branch yourself.
- A rejected push ends the turn; no untested merge is silently retried.
- A standalone gate before review lands nothing. Only the landing worker of a bound workflow takes the turn after review.
- A conflict remains in progress. Commit its resolution before asking again."#;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The words the turn's own code sets as `outcome`: each through an
    /// ending, never as a bare string — its own, or the one the chain
    /// decided (`candidate::Decision::ending`).
    fn returned() -> BTreeSet<&'static str> {
        let code = [
            include_str!("turn.rs"),
            include_str!("candidates.rs"),
            include_str!("../../core/src/candidate.rs"),
        ];
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

    /// A base tip gated alone red says how in the one red wording, as a
    /// candidate's `gate_red` does (sunfmin/control#619): its `base_red`
    /// answer and its log line carry it.
    #[test]
    fn a_base_tip_gated_red_is_told_by_the_one_red_wording() {
        let lines: Vec<String> =
            ["        FAIL [   0.208s] (2/3) nx a_test".to_string(), "end".to_string()].to_vec();
        let afkconvoy_core::gate::Judged::Red(red) =
            afkconvoy_core::gate::judge(1, false, 600, None, &lines)
        else {
            panic!("a FAIL is red");
        };
        assert_eq!(
            red_tip("abc", &red),
            "base tip abc is red: it exited 1; failing: nx a_test; worker turns wait for its repair"
        );
        assert!(red_tip("abc", &red).starts_with(&red.said("base tip abc")));
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

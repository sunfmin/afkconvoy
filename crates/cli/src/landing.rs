//! Recorded landings: proof-checked pushes and recovery from merge history.
use crate::gh::Tracker;
use crate::git::Repo;
use crate::remote::Push;
use afkconvoy_core::gate::Proven;
use afkconvoy_core::landing::{LandedPr, Logged, read_landings};
use anyhow::{Result, bail};
/// Pushes `commit` to the base branch as a fast-forward → whether the remote
/// took it. It asks for the proof of the commit's tree, and pushes nothing
/// that proof is not of.
pub fn push_base(repo: &Repo, branch: &str, commit: &str, proven: &Proven) -> Result<bool> {
    let tree = repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])?;
    if tree != proven.tree() {
        bail!(
            "{commit} does not hold the tree the gate proved ({}); nothing landed",
            proven.tree()
        );
    }
    Ok(repo.remote().try_push(&Push::set(commit, &format!("refs/heads/{branch}")))?.ok)
}

/// The joinings among the commits `revs` selects, oldest first, as the fleet
/// whose tracker is `tracker` reads them. Whatever narrows the walk
/// (`--grep`) never decides what a commit is. A merge is not required: a
/// cut's landing has one parent, and `read_landings` is what admits it.
pub fn joins_in(repo: &Repo, tracker: &Tracker, revs: &[&str]) -> Result<Vec<LandedPr>> {
    let mut args = vec!["log", "--reverse", "--format=%H%x09%P%x09%B%x00"];
    args.extend(revs);
    let log = repo.git_raw(&args)?;
    let commits: Vec<Logged> = log
        .split('\0')
        .filter_map(|entry| {
            let mut parts = entry.trim_matches('\n').splitn(3, '\t');
            Some(Logged { sha: parts.next()?, parents: parts.next()?, message: parts.next()? })
        })
        .collect();
    Ok(read_landings(&commits, tracker))
}

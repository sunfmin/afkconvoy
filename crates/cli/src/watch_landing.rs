//! A watch lands its own change in a turn, on no issue.
use crate::{gh::GitHub, git::Repo, host_watch::CHECKOUT, orca::Worktree};
use afkconvoy_core::{config::Config, landing::Of};
use anyhow::Result;
use serde_json::{Value, json};

pub fn land(repo: &Repo, config: &Config, gh: &GitHub, host: &str, now: u64) -> Result<Value> {
    let checkout = Worktree::of(repo)?;
    let named = checkout.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if !afkconvoy_core::worker::is_worktree_named(named, crate::host_watch::CHECKOUT) {
        anyhow::bail!(
            "{} is no watch's checkout: land from your own \
             linked worktree named `{CHECKOUT}`, optionally followed by -<digits>",
            checkout.path.display(),
        );
    }
    let Some(branch) = checkout.branch_now()? else {
        anyhow::bail!(
            "{} is on no branch: commit the change on the branch of this checkout, push it and \
             open its PR, then run this again",
            checkout.path.display()
        );
    };
    let prs = gh.open_prs()?;
    let Some(pr) = prs.iter().find(|pr| pr.head_branch == branch) else {
        anyhow::bail!(
            "no open PR has this checkout's branch {branch:?} for its head — push it \
             (`git push origin HEAD`) and open one (`gh pr create --base {} --head {branch}`), \
             then run this again",
            config.base_branch
        );
    };
    let answer = json!({"watch": host, "pr": pr.number});
    let locks = crate::lock::standing_locks(gh, config)?;
    if !locks.is_empty() {
        return Ok(crate::turn::locked(answer, &locks));
    }
    let of = Of::Watch(host.to_string());
    crate::turn::join(repo, config, &gh.tracker, pr, &of, answer, (host, now))
}

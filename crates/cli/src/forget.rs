//! Letting go of a claim whose issue is gone: GitHub says it was deleted,
//! transferred to another repo, or never issued. Like a closed issue it is
//! never worked again; unlike one, nothing is left on GitHub to settle.

use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition::{self, ForgetStep};
use anyhow::Result;

use crate::claims::{Me, delete};
use crate::fleet::Fleet;
use crate::found::Found;
use crate::gh::{Gone, gone};
use crate::judge::{leaving, mine, worktree_of};

/// Whether issue `number` is gone, as GitHub says now → how: `None` of one it
/// still has. Any other failure to read it is an error.
pub fn asked(fleet: &Fleet, number: u64) -> Result<Option<Gone>> {
    match fleet.gh.issue(number) {
        Ok(_) => Ok(None),
        Err(error) => gone(&error).cloned().map(Some).ok_or(error),
    }
}

/// Lets go of my claim on `issue`, which GitHub says is gone: release it →
/// remove its worktree here when that holds nothing the remote does not, or
/// close its worker's terminal in one kept for what only it holds. The
/// issue is asked about again first: one GitHub has after all is left as it
/// is. `found` notes it, for the tick's line.
pub fn forget(fleet: &mut Fleet, found: &mut Found, me: &Me, issue: u64) -> Result<Outcome> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(Outcome::Nothing) };
    let Some(how) = asked(fleet, issue)? else { return Ok(Outcome::Nothing) };
    let worktree = worktree_of(fleet, issue)?;
    let left = worktree.as_ref().map(|worktree| leaving(fleet, issue, worktree)).transpose()?;
    let spare = left.as_ref().and_then(|left| left.left.spare());
    transition::forget(issue, spare, |step| {
        match (step, &worktree) {
            (ForgetStep::Release, _) => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            (ForgetStep::RemoveWorktree, Some(worktree)) => worktree.remove()?,
            (ForgetStep::CloseTerminal, Some(worktree)) => worktree.close_terminals()?,
            (_, None) => {}
        }
        Ok::<(), anyhow::Error>(())
    })?;
    found.forgotten.push((how, true));
    Ok(Outcome::Done)
}

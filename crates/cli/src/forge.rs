//! afkconvoy's forge, as a landing asks it (docs/adr/0060): a GitHub pull
//! request is the change that asks to land, the issue it closes what it
//! completes, the issue's open blockers and pinned target what may keep it
//! off, and closing that issue, with its work account, what follows once it
//! landed. The landing's rules are the landing crate's; this is the one
//! implementation of its `Forge` afkconvoy has.

use afkconvoy_core::account::End;
use afkconvoy_core::candidate::Note;
use afkconvoy_core::config::Config;
use afkconvoy_core::landing::{Of, merge_message};
use afkconvoy_core::plan::PullRequest;
use afkconvoy_landing::forge::{Change, Forge};
use anyhow::Result;

use crate::gh::{GitHub, Tracker};
use crate::git::Repo;

/// One turn's forge: the repo it lands in, routed to its target, and whose
/// PR it is.
pub(crate) struct Pulls<'a> {
    pub repo: &'a Repo,
    pub config: &'a Config,
    pub gh: &'a GitHub,
    pub tracker: &'a Tracker,
    pub of: &'a Of,
    /// The time now, in epoch seconds: a turn's clock.
    pub clock: &'a dyn Fn() -> u64,
}

/// Why a PR may not land now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The issue it closes is blocked by these open issues.
    Blocked(Vec<u64>),
    /// The issue's pinned target is no longer the branch it lands on.
    Retargeted,
}

/// A PR as the change it asks to land.
pub(crate) fn change(pr: &PullRequest) -> Change {
    Change {
        number: pr.number,
        source: pr.head_branch.clone(),
        target: pr.base_branch.clone(),
        title: pr.title.clone(),
    }
}

impl Pulls<'_> {
    /// The issue of the fleet's tracker the PR closes, where it is a worker's.
    pub fn issue(&self) -> Option<u64> {
        match self.of {
            Of::Issue(n) => Some(*n),
            Of::Elsewhere(_) | Of::Watch(_) | Of::Cut(_) => None,
        }
    }
}

impl Forge for Pulls<'_> {
    type Error = anyhow::Error;
    type Note = Note;
    type Refusal = Refusal;

    fn current(&self, change: &Change) -> Result<Option<Change>> {
        Ok(self.gh.pr_of_branch(&change.source)?.as_ref().map(self::change))
    }

    fn note(&self, _: &Change) -> Note {
        Note { issue: self.issue() }
    }

    fn message(&self, change: &Change) -> String {
        merge_message(&change.title, change.number, self.of, self.tracker)
    }

    fn may_land(&self, _: &Change) -> Result<Result<(), Refusal>> {
        let Some(n) = self.issue() else { return Ok(Ok(())) };
        let blockers = self.gh.open_blockers(n)?;
        if !blockers.is_empty() {
            return Ok(Err(Refusal::Blocked(blockers)));
        }
        if crate::target::branch(self.repo, self.config, n)? != self.config.base_branch {
            return Ok(Err(Refusal::Retargeted));
        }
        Ok(Ok(()))
    }

    fn landed(&self, _: &Change, _commit: &str) -> Result<()> {
        let Some(n) = self.issue() else { return Ok(()) };
        self.gh.close_issue(n)?;
        crate::account::leave(self.repo, self.gh, n, End::Landed, (self.clock)())
    }
}

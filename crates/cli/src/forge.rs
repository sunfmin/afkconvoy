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
use afkconvoy_core::worker::{carried, carried_comment};
use afkconvoy_landing::forge::{Change, Forge};
use anyhow::Result;
use std::collections::BTreeMap;

use crate::gh::{GitHub, Tracker};
use crate::git::Repo;
use crate::remote::Push;

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
    /// The issue is closed: nothing lands for it (sunfmin/control#1081).
    Closed(u64),
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
    /// The open issues of the fleet's tracker whose latest record says PR
    /// `by` carries their PR (`carried_comment`), each with that PR. Where
    /// the open issues cannot be read — GitHub's GraphQL is down, say — none
    /// is told, with a warning: a landing never waits on them, its
    /// description still names what it carried, and doctor names a carried
    /// issue left open once its carrier landed (`carried_landed`).
    fn told_carried(&self, by: u64) -> Result<BTreeMap<u64, u64>> {
        let open = match crate::issues::of(self.gh).open_issues(None) {
            Ok(open) => open.issues,
            Err(error) => {
                eprintln!(
                    "[afkconvoy] land: the issues PR #{by} was told it carries cannot be read, so its description alone says them: {error:#}"
                );
                return Ok(BTreeMap::new());
            }
        };
        let mut told = BTreeMap::new();
        for issue in &open.rows {
            let bodies = open.comments(issue.number).iter().map(|c| c.body.as_str());
            if let Some((pr, carrier)) = carried(bodies)
                && carrier == by
            {
                told.insert(issue.number, pr);
            }
        }
        Ok(told)
    }

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
        let issues = crate::issues::of(self.gh);
        let Some(n) = self.issue() else { return Ok(Ok(())) };
        if issues.issue(n)?.is_closed() {
            return Ok(Err(Refusal::Closed(n)));
        }
        let blockers = issues.open_blockers(n)?;
        if !blockers.is_empty() {
            return Ok(Err(Refusal::Blocked(blockers)));
        }
        if crate::target::branch(self.repo, self.config, n)? != self.config.base_branch {
            return Ok(Err(Refusal::Retargeted));
        }
        Ok(Ok(()))
    }

    /// Its issue closes, and so does each issue of a PR it carried: each its
    /// description closes as well (docs/adr/0083), and each the fleet told
    /// it carries, whatever its description says now (sunfmin/control#1248).
    fn landed(&self, change: &Change, _commit: &str) -> Result<()> {
        let issues = crate::issues::of(self.gh);
        let Some(n) = self.issue() else { return Ok(()) };
        issues.close_issue(n)?;
        crate::account::leave(self.repo, self.gh, n, End::Landed, (self.clock)())?;
        let body = self.gh.pr_body(change.number)?;
        let mut carried: Vec<u64> = self.told_carried(change.number)?.into_keys().collect();
        if !afkconvoy_landing::forge::carried_in(&body).is_empty() {
            carried.extend(self.tracker.closed_by(&body, &[], None));
        }
        carried.sort_unstable();
        carried.dedup();
        for carried in carried.into_iter().filter(|&c| c != n) {
            if !issues.issue(carried)?.is_closed() {
                issues.close_issue(carried)?;
                crate::account::leave(self.repo, self.gh, carried, End::Landed, (self.clock)())?;
            }
        }
        Ok(())
    }

    /// Each PR whose issue the fleet told is carried by `change`
    /// (`carried_comment`), as `carry` was handed it.
    fn carries(&self, change: &Change) -> Result<Vec<Change>> {
        let mut prs: Vec<u64> = self.told_carried(change.number)?.into_values().collect();
        prs.sort_unstable();
        prs.dedup();
        let mut carries = vec![];
        for pr in prs {
            carries.push(Change {
                number: pr,
                source: String::new(),
                target: change.target.clone(),
                title: self.gh.pr_title(pr)?,
            });
        }
        Ok(carries)
    }

    fn description(&self, change: &Change) -> Result<String> {
        self.gh.pr_body(change.number)
    }

    fn describe(&self, change: &Change, description: &str) -> Result<()> {
        self.gh.edit_pr_body(change.number, description)
    }

    fn close(&self, change: &Change, comment: &str) -> Result<()> {
        self.gh.close_pr(change.number, comment)
    }

    /// A PR headed by a full ref has no branch of its own to delete.
    fn delete_source(&self, change: &Change, head: &str) -> Result<()> {
        if change.source.starts_with("refs/") {
            return Ok(());
        }
        let branch = format!("refs/heads/{}", change.source);
        self.repo.remote().push(&Push::delete(&branch).leased(head))
    }

    /// Each issue the carried PR closes is told it is carried, and on which
    /// branch its work goes on: the fleet then ends the worker of a claim on
    /// it and removes its worktree (`plan::Status::Carried`), holds it off
    /// while `by` is open, and starts its next worker from that branch
    /// should `by` close unlanded.
    fn carried(&self, carried: &Change, by: &Change) -> Result<()> {
        let issues = crate::issues::of(self.gh);
        let body = self.gh.pr_body(carried.number)?;
        for issue in self.tracker.closed_by(&body, &[], None) {
            issues.comment(issue, &carried_comment(carried.number, by.number, &by.source))?;
        }
        Ok(())
    }
}

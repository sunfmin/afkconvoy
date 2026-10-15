//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.
//!
//! A rebuild asks the remote once and GitHub a fixed number of times, side by
//! side: one listing of the base branch and every ref of the fleet
//! (`Repo::listing`), and what is open on GitHub (`GitHub::open`). Neither
//! grows with the issues in flight.

use afkconvoy_core::claims::{Claim, InstanceId};
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::{Observed, WorkingSet, assemble};
use afkconvoy_core::stopped::standing_verdict;
use afkconvoy_core::train::on_train;
use afkconvoy_core::verdict::latest_verdict;
use anyhow::{Result, anyhow};
use std::thread;

use crate::claims::Scan;
use crate::config;
use crate::gh::{GitHub, Open, beside};
use crate::git::{Listing, Repo};
use crate::orca::{Source, Worktree};
use crate::patrol;
use crate::precedent;
use crate::settle::cut_landing_of;
use crate::slow_gate;
use crate::stage::Flows;
use crate::train::Train;

pub struct Fleet {
    pub repo: Repo,
    pub config: Config,
    pub gh: GitHub,
    pub scan: Scan,
    /// The base branch and the fleet's refs as the remote had them when this
    /// view was opened: what `scan` was read from.
    listing: Listing,
    pub now: u64,
    /// The issues this view escalated, each with why in a few words: what a
    /// tick's line says of them. Every escalation is made through
    /// `judge::escalate_claim`, which is what adds to it.
    pub escalated: Vec<(u64, String)>,
}

impl Fleet {
    /// The fleet's view from the checkout the command was run in.
    pub fn open(now: u64) -> Result<Fleet> {
        let repo = Repo::here()?;
        let config = config::load(&repo.dir.join(config::FILE))?;
        let listing = repo.listing(&config.base_branch)?;
        let scan = Scan::among(repo.layout(), listing.records.clone());
        let gh = GitHub { dir: repo.dir.clone() };
        Ok(Fleet { repo, config, gh, scan, listing, now, escalated: vec![] })
    }

    /// The fleet's view, opened for a rebuild → it, and what is open on
    /// GitHub. The remote and GitHub do not depend on each other: they are
    /// asked side by side.
    pub fn read(now: u64) -> Result<(Fleet, Open)> {
        let gh = GitHub { dir: Repo::here()?.dir };
        let (fleet, open) = thread::scope(|scope| {
            let open = scope.spawn(|| gh.open());
            (Fleet::open(now), beside(open))
        });
        Ok((fleet?, open?))
    }

    /// The base branch's tip on the remote, its commits in this repo.
    pub fn base_tip(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        self.repo
            .fetched(&format!("refs/heads/{branch}"))?
            .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))
    }

    /// Rebuilds the working set of `me` from `open` and the remote as this
    /// view was opened on it (`Fleet::read`) → it, and the read of the issues
    /// that follow a workflow it was assembled from. Reads only.
    pub fn rebuild(&self, open: &Open, me: Option<&InstanceId>) -> Result<(WorkingSet, Flows)> {
        let (issues, prs) = (&open.issues.rows, &open.prs);
        let stages = Flows::read(self, &open.issues)?;
        // The list holds only open issues: a claim missing from it is asked about.
        let mut closed = vec![];
        for claim in &self.scan.claims {
            if !issues.iter().any(|issue| issue.number == claim.number)
                && self.gh.issue(claim.number)?.is_closed()
            {
                closed.push(claim.number);
            }
        }
        // What is on the train is read off the train's own commits.
        let train = Train::listed(&self.repo, &self.config, &self.listing)?;
        let joins = train.joins(&self.repo)?;
        // …and what a cut landing left open, off the base branch.
        let mut landed = vec![];
        for claim in &self.scan.claims {
            if me.is_some_and(|me| claim.is_held_by(me))
                && !closed.contains(&claim.number)
                && cut_landing_of(&self.repo, &train.base, claim, prs)?.is_some()
            {
                landed.push(claim.number);
            }
        }
        let joined: Vec<u64> = on_train(&joins, prs).iter().map(|join| join.issue).collect();
        let mut declared = vec![];
        for claim in
            self.scan.claims.iter().filter(|claim| me.is_some_and(|me| claim.is_held_by(me)))
        {
            let under = match &self.config.bound {
                Some(_) => stages.declared(claim.number, &claim.id),
                None => self.declared_since(open, claim),
            };
            if under {
                declared.push(claim.number);
            }
        }
        let heartbeats = self.scan.beats();
        let (landings, slow_gate) = slow_gate::seen(self, &self.listing.records, &train.base)?;
        let seen = Observed {
            issues,
            prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
            joined: &joined,
            landed: &landed,
            declared: &declared,
            due: &stages.due(prs),
            landing: &stages.landing(&self.scan.claims),
            begun: &stages.begun(),
            patrols: &patrol::seen(self, &stages, &open.issues)?,
            proposals: &precedent::seen(self, &stages)?,
            waiting: &stages.waiting(self, open),
            landings: &landings,
            slow_gate: &slow_gate,
        };
        let set = assemble(&seen, me, self.now, &self.config);
        Ok((set, stages))
    }

    /// Whether the worker of `claim`, in an unbound repo, declared a verdict
    /// that stands: the latest on the issue, written since the claim was
    /// created — one from an earlier claim is not its own — and not answered
    /// by a worker started on the issue again (`stopped::standing_verdict`).
    /// What cannot be read says no: the claim then takes a slot.
    fn declared_since(&self, open: &Open, claim: &Claim) -> bool {
        let comments = open.issues.comments(claim.number);
        let Some(verdict) = latest_verdict(comments.iter().map(|c| c.body.as_str())) else {
            return false;
        };
        let Some(said) = comments.get(verdict.comment) else { return false };
        let since = said.at.zip(claim.since).is_some_and(|(at, since)| at >= since);
        // Only a verdict that is the claim's own is looked for in its worktree.
        let answered = || -> Option<Option<String>> {
            let worktree =
                Worktree::of_issue(&Source::of(&self.repo).ok()?, claim.number).ok()??;
            worktree.answered().ok()
        };
        since
            && answered().is_some_and(|answered| {
                standing_verdict(Some(verdict), Some(&said.url), answered.as_deref()).is_some()
            })
    }
}

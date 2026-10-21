//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.
//!
//! A rebuild asks the remote once and GitHub a fixed number of times, side by
//! side: one listing of the base branch and every ref of the fleet
//! (`Repo::listing`), and what is open on GitHub (`GitHub::open`). Neither
//! grows with the issues in flight.

use afkconvoy_core::claims::{Claim, InstanceId};
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::{Issue, Observed, WorkingSet, assemble, closing_pr};
use afkconvoy_core::stock;
use afkconvoy_core::stopped::standing_verdict;
use afkconvoy_core::train::{Built, on_train};
use afkconvoy_core::verdict::latest_verdict;
use afkconvoy_core::worker::recorded_branches;
use anyhow::{Result, anyhow};
use std::thread;

use crate::claims::Scan;
use crate::config;
use crate::gh::{GitHub, Gone, Open, beside, gone};
use crate::git::{Listing, Repo};
use crate::measure;
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
    /// The issues this view found gone — deleted or transferred — as a step
    /// met them, each with whether a claim on it was released: what a tick's
    /// line says of them.
    pub forgotten: Vec<(Gone, bool)>,
    /// The claimed issues the last rebuild could not read, each with why: the
    /// pass goes on with them as open, and its tick reports each.
    pub unread: Vec<(u64, anyhow::Error)>,
    /// The closed claimed issues the last rebuild read, each with the login of
    /// whoever closed it, where GitHub says: what a tick's line names.
    pub closers: Vec<(u64, String)>,
    /// What this machine's train's worktree held when this view cut it again:
    /// what a tick's line says of it.
    pub cut_train: Option<Built>,
}

impl Fleet {
    /// The fleet's view from the checkout the command was run in.
    pub fn open(now: u64) -> Result<Fleet> {
        let repo = Repo::here()?;
        let config = config::load(&repo.dir.join(config::FILE))?;
        let listing = repo.listing(&config.base_branch)?;
        let scan = Scan::among(repo.layout(), listing.records.clone());
        let gh = GitHub { dir: repo.dir.clone() };
        Ok(Fleet {
            repo,
            config,
            gh,
            scan,
            listing,
            now,
            escalated: vec![],
            forgotten: vec![],
            unread: vec![],
            closers: vec![],
            cut_train: None,
        })
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
    /// that follow a workflow it was assembled from. Reads only. A claimed
    /// issue that cannot be read costs only itself: it is taken for open,
    /// and noted in `unread`.
    pub fn rebuild(&mut self, open: &Open, me: Option<&InstanceId>) -> Result<(WorkingSet, Flows)> {
        let (issues, prs) = (&open.issues.rows, &open.prs);
        let stages = Flows::read(self, &open.issues)?;
        // The list holds only open issues: a claim missing from it is asked
        // about, and is closed, gone — deleted or transferred — or unread.
        let (mut closed, mut deleted) = (vec![], vec![]);
        self.unread.clear();
        self.closers.clear();
        for claim in &self.scan.claims {
            if issues.iter().any(|issue| issue.number == claim.number) {
                continue;
            }
            match self.gh.issue(claim.number) {
                Ok(read) if read.is_closed() => {
                    closed.push(claim.number);
                    self.closers.extend(read.closed_by.map(|by| (claim.number, by)));
                }
                Ok(_) => {}
                Err(error) if gone(&error).is_some() => deleted.push(claim.number),
                Err(error) => self.unread.push((claim.number, error)),
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
        // A PR is the fleet's own when its branch is one recorded on the issue.
        let mut kept = vec![];
        for issue in issues {
            let bodies = open.issues.comments(issue.number).iter().map(|c| c.body.as_str());
            if closing_pr(prs, issue.number)
                .is_some_and(|pr| recorded_branches(bodies).contains(&pr.head_branch))
            {
                kept.push(issue.number);
            }
        }
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
            gone: &deleted,
            joined: &joined,
            kept: &kept,
            landed: &landed,
            declared: &declared,
            due: &stages.due(prs),
            flows: &stages.flow_labels(),
            landing: &stages.landing(&self.scan.claims),
            begun: &stages.begun(),
            patrols: &patrol::seen(self, &stages, &open.issues)?,
            proposals: &precedent::seen(self, &stages)?,
            waiting: &stages.waiting(self, open),
            landings: &landings,
            slow_gate: &slow_gate,
            idle_train: if train.tip == train.base { self.train_built() } else { None },
            consults: &self.stock_consults(open),
        };
        let set = assemble(&seen, me, self.now, &self.config);
        Ok((set, stages))
    }

    /// What the last landing measured of this machine's train's worktree;
    /// `None` when there is none here, no landing measured it, or orca or the
    /// worktree cannot be read — nothing is cut on what is not known.
    fn train_built(&self) -> Option<Built> {
        let worktree = Worktree::of_train(&Source::of(&self.repo).ok()?).ok()??;
        measure::measured(&Repo::new(worktree.path)).ok()?
    }

    /// The open issues of an unbound repo that are the stock consult's
    /// (`plan::Observed::consults`): escalated and unclaimed with the consult
    /// due, or claimed under a claim it was asked under.
    fn stock_consults(&self, open: &Open) -> Vec<u64> {
        if !self.config.stock_consult() {
            return vec![];
        }
        let consults = |issue: &&Issue| {
            let bodies = || open.issues.comments(issue.number).iter().map(|c| c.body.as_str());
            match self.scan.claim_of(issue.number) {
                // Whatever labels its answer left.
                Some(claim) => stock::asked_under(bodies(), &claim.id),
                None => issue.labels.contains(&self.config.escalate_label) && stock::due(bodies()),
            }
        };
        open.issues.rows.iter().filter(consults).map(|issue| issue.number).collect()
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

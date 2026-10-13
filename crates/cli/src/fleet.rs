//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.
//!
//! A rebuild asks the remote once and GitHub a fixed number of times, side by
//! side: one listing of the base branch and every ref of the fleet
//! (`Repo::listing`), and what is open on GitHub (`GitHub::open`). Neither
//! grows with the issues in flight.

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::{Observed, WorkingSet, assemble};
use afkconvoy_core::train::on_train;
use anyhow::{Result, anyhow};
use std::thread;

use crate::claims::Scan;
use crate::config;
use crate::gh::{GitHub, Open, beside};
use crate::git::{Listing, Repo};
use crate::patrol;
use crate::precedent;
use crate::settle::cut_landing_of;
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
}

impl Fleet {
    /// The fleet's view from the checkout the command was run in.
    pub fn open(now: u64) -> Result<Fleet> {
        let repo = Repo::here()?;
        let config = config::load(&repo.dir.join(config::FILE))?;
        let listing = repo.listing(&config.base_branch)?;
        let scan = Scan::among(repo.layout(), listing.records.clone());
        let gh = GitHub { dir: repo.dir.clone() };
        Ok(Fleet { repo, config, gh, scan, listing, now })
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
        let heartbeats = self.scan.beats();
        let seen = Observed {
            issues,
            prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
            joined: &joined,
            landed: &landed,
            due: &stages.due(prs),
            landing: &stages.landing(&self.scan.claims),
            begun: &stages.begun(),
            patrols: &patrol::seen(self, &stages, &open.issues)?,
            proposals: &precedent::seen(self, &stages)?,
        };
        let set = assemble(&seen, me, self.now, &self.config);
        Ok((set, stages))
    }
}

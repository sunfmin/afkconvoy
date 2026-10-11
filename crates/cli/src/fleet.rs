//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::{Observed, WorkingSet, assemble};
use afkconvoy_core::train::on_train;
use anyhow::{Result, anyhow};

use crate::claims::Scan;
use crate::config;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::settle::cut_landing_of;
use crate::train::Train;

pub struct Fleet {
    pub repo: Repo,
    pub config: Config,
    pub gh: GitHub,
    pub scan: Scan,
    pub now: u64,
}

impl Fleet {
    /// The fleet's view from the checkout the command was run in.
    pub fn open(now: u64) -> Result<Fleet> {
        let repo = Repo::here()?;
        let config = config::load(&repo.dir.join(config::FILE))?;
        let scan = Scan::read(&repo)?;
        let gh = GitHub { dir: repo.dir.clone() };
        Ok(Fleet { repo, config, gh, scan, now })
    }

    /// The base branch's tip on the remote, its commits in this repo.
    pub fn base_tip(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        self.repo
            .fetched(&format!("refs/heads/{branch}"))?
            .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))
    }

    /// Rebuilds the working set of `me` from what GitHub and the remote say
    /// now. Reads only.
    pub fn rebuild(&self, me: Option<&InstanceId>) -> Result<WorkingSet> {
        let issues = self.gh.open_issues()?;
        let prs = self.gh.open_prs()?;
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
        let train = Train::read(&self.repo, &self.config)?;
        let joins = train.joins(&self.repo)?;
        // …and what a cut landing left open, off the base branch.
        let mut landed = vec![];
        for claim in &self.scan.claims {
            if me.is_some_and(|me| claim.is_held_by(me))
                && !closed.contains(&claim.number)
                && cut_landing_of(&self.repo, &train.base, claim, &prs)?.is_some()
            {
                landed.push(claim.number);
            }
        }
        let joined: Vec<u64> = on_train(&joins, &prs).iter().map(|join| join.issue).collect();
        let heartbeats: Vec<(String, u64)> =
            self.scan.heartbeats.iter().map(|(instance, ts)| (instance.clone(), *ts)).collect();
        let seen = Observed {
            issues: &issues,
            prs: &prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
            joined: &joined,
            landed: &landed,
        };
        Ok(assemble(&seen, me, self.now, &self.config))
    }
}

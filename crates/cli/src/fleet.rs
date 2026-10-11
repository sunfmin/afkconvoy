//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::{Observed, WorkingSet, assemble};
use anyhow::Result;

use crate::claims::Scan;
use crate::config;
use crate::gh::GitHub;
use crate::git::Repo;

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
        let heartbeats: Vec<(String, u64)> =
            self.scan.heartbeats.iter().map(|(instance, ts)| (instance.clone(), *ts)).collect();
        let seen = Observed {
            issues: &issues,
            prs: &prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
        };
        Ok(assemble(&seen, me, self.now, &self.config))
    }
}

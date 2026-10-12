//! Starting a worker in its role (docs/adr/0009): the stage a bound repo's
//! issue is dispatched to, the copies its worker starts among, and what a
//! start asks of the worker launch command.

use std::path::PathBuf;

use afkconvoy_core::config::Bound;
use afkconvoy_core::prompt::{StageFields, stage_prompt};
use afkconvoy_core::role::{Facts, Start, first_stage, in_role, quoted, takes_flags};
use afkconvoy_core::team::PRECEDENTS;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::copies::{Copies, Copy};
use crate::fleet::Fleet;
use crate::launch::Proposed;
use crate::orca::Worktree;
use crate::team::keep;

/// One stage of one issue, as its worker is started on it: the role and the
/// team at the commits fixed for the stage.
pub struct Staged {
    issue: u64,
    /// The workflow's name.
    flow: String,
    stage: String,
    role: String,
    skill: String,
    writes: bool,
    model: Option<String>,
    role_copy: Copy,
    team_copy: Copy,
}

impl Staged {
    /// The stage issue `issue`, carrying `labels`, enters its workflow at,
    /// with the team and its roles as they stand now — each copied here
    /// unless a copy of that commit already is. An error is an issue no
    /// worker can be started on, or a team that does not load.
    pub fn enter(fleet: &Fleet, bound: &Bound, issue: u64, labels: &[String]) -> Result<Staged> {
        let kept = keep(&fleet.gh, &Copies::here()?, &bound.team)?;
        let entry = first_stage(&kept.team, labels)
            .map_err(|why| anyhow!("issue #{issue} cannot be started: {why}"))?;
        Ok(Staged {
            issue,
            flow: entry.workflow.name.clone(),
            stage: entry.stage.name.clone(),
            role: entry.work.role.clone(),
            skill: entry.work.skill.clone(),
            writes: entry.stage.writes,
            model: entry.stage.model.clone(),
            role_copy: kept.role(entry.role)?.clone(),
            team_copy: kept.copy.clone(),
        })
    }

    /// The line the terminal on `worktree` runs to start the worker with the
    /// worker launch command `launch`.
    pub fn command(&self, launch: &str, worktree: &Worktree) -> String {
        let start = Start {
            role_dir: &self.role_copy.dir.display().to_string(),
            worktree: &worktree.path.display().to_string(),
            team_dir: &self.team_copy.dir.display().to_string(),
            model: self.model.as_deref(),
            facts: Facts {
                issue: self.issue,
                flow: &self.flow,
                stage: &self.stage,
                role: &self.role,
                skill: &self.skill,
                writes: self.writes,
                role_commit: &self.role_copy.commit,
                team_commit: &self.team_copy.commit,
            },
        };
        in_role(launch, &start)
    }

    /// Writes the worker's brief into `worktree` → the file.
    pub fn write_brief(&self, fleet: &Fleet, worktree: &Worktree, title: &str) -> Result<PathBuf> {
        let verdict = format!("verdict --issue {} --stage {}", self.issue, quoted(&self.stage));
        let fields = StageFields {
            number: self.issue,
            title,
            flow: &self.flow,
            stage: &self.stage,
            role: &self.role,
            skill: &self.skill,
            branch: &worktree.branch,
            worktree: &worktree.path.display().to_string(),
            base_branch: &fleet.config.base_branch,
            precedents: &self.team_copy.dir.join(PRECEDENTS).display().to_string(),
            verdict_command: &crate::own_command(&verdict),
        };
        worktree.write_brief(&stage_prompt(&fields).map_err(|e| anyhow!(e))?)
    }

    pub fn json(&self) -> Value {
        json!({"flow": self.flow, "name": self.stage, "role": self.role, "skill": self.skill,
            "writes": self.writes, "model": self.model,
            "role_commit": self.role_copy.commit, "role_dir": self.role_copy.dir,
            "team_commit": self.team_copy.commit, "team_dir": self.team_copy.dir})
    }
}

/// Checks that the worker launch command `launch` takes what a bound repo's
/// fleet adds to it, asking the login shell a worker gets. An error names
/// what it does not take.
pub fn probe(launch: &Proposed) -> Result<()> {
    takes_flags(launch.as_str(), launch.flags().as_deref()).map_err(|why| anyhow!(why))
}

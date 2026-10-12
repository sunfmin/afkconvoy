//! Starting a worker in its role (docs/adr/0009): the stage a bound repo's
//! issue is dispatched to, the copies its worker starts among, and what a
//! start asks of the worker launch command.

use std::path::PathBuf;

use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::Bound;
use afkconvoy_core::prompt::{Said, StageFields, stage_prompt};
use afkconvoy_core::role::{Due, Facts, Start, due, in_role, quoted, takes_flags};
use afkconvoy_core::stage_record::{Reading, StageRecord, history};
use afkconvoy_core::team::PRECEDENTS;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::copies::{Copies, Copy};
use crate::fleet::Fleet;
use crate::launch::Proposed;
use crate::orca::Worktree;
use crate::stage::records_of;
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
    /// The stage records that counted on the issue so far, oldest first,
    /// each with the URL of its comment: the worker's history.
    verdicts: Vec<(StageRecord, String)>,
}

impl Staged {
    /// The stage issue `issue`, carrying `labels`, is at by its stage
    /// records as they stand now, with the team and its roles as they stand
    /// now — each copied here unless a copy of that commit already is. An
    /// error is an issue no worker can be started on, or a team that does
    /// not load.
    pub fn enter(fleet: &Fleet, bound: &Bound, issue: u64, labels: &[String]) -> Result<Staged> {
        let kept = keep(&fleet.gh, &Copies::here()?, &bound.team)?;
        let (records, urls) = records_of(&fleet.gh, issue)?;
        let entry = match due(&kept.team, labels, &records) {
            Due::Worker(entry) => entry,
            Due::Human(why) | Due::Nothing(why) => {
                bail!("issue #{issue} cannot be started: {why}")
            }
        };
        let readings = history(entry.workflow, &records).readings;
        let verdicts = records.into_iter().zip(urls).zip(readings);
        Ok(Staged {
            verdicts: verdicts
                .filter(|(_, reading)| matches!(reading, Reading::Verdict | Reading::Sent))
                .map(|(verdict, _)| verdict)
                .collect(),
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

    /// Refuses a start under `claim` when a verdict was already declared
    /// under it: a claim declares once, so the stage started would have no
    /// say. The tick ends such a claim, and the next start takes another.
    pub fn refuse_spent(&self, claim: Option<&Claim>) -> Result<()> {
        let spent = |claim: &&Claim| {
            self.verdicts.iter().any(|(said, _)| said.claim() == Some(claim.sha.as_str()))
        };
        match claim.filter(spent) {
            Some(_) => bail!(
                "issue #{}: a stage already declared under the claim held on it, and a claim \
                 declares once; the stage `{}` is started under the next one",
                self.issue,
                self.stage
            ),
            None => Ok(()),
        }
    }

    /// Writes the worker's brief into `worktree` → the file.
    pub fn write_brief(&self, fleet: &Fleet, worktree: &Worktree, title: &str) -> Result<PathBuf> {
        let verdict = format!("verdict --issue {} --stage {}", self.issue, quoted(&self.stage));
        let records: Vec<Said> =
            self.verdicts.iter().map(|(record, url)| Said { record, url }).collect();
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
            records: &records,
            // A worker starts in its role's copy: the command is run from the worktree.
            verdict_command: &format!(
                "cd {} && {}",
                quoted(&worktree.path.display().to_string()),
                crate::own_command(&verdict)
            ),
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

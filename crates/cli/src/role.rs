//! Starting a worker in its role (docs/adr/0009): the stage a bound repo's
//! issue is dispatched to, the copies its worker starts among, and what a
//! start asks of the worker launch command.

use std::path::PathBuf;

use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::Bound;
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::{
    Landing, Said, StageFields, WorkerFields, landing_prompt, stage_prompt,
};
use afkconvoy_core::role::{Due, Facts, Start, due, in_role, quoted, takes_flags};
use afkconvoy_core::stage_record::{Reading, StageRecord, history};
use afkconvoy_core::team::{LAND, PRECEDENTS};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::copies::{Copies, Copy};
use crate::fleet::Fleet;
use crate::launch::Proposed;
use crate::orca::Worktree;
use crate::stage::records_of;
use crate::team::keep;

/// One stage of one issue, as its worker is started on it — or the join that
/// follows the last one: the role and the team at the commits fixed for it.
pub struct Staged {
    issue: u64,
    /// The workflow's name.
    flow: String,
    /// The stage's name; `land` for the join.
    stage: String,
    role: String,
    skill: String,
    writes: bool,
    model: Option<String>,
    /// What the guard holds every worker to: the repo's base branch, the
    /// team's escalate label and the repo's lock label.
    base_branch: String,
    escalate_label: String,
    lock_label: String,
    /// The PR a landing worker gates and joins. `None` at a stage.
    joins: Option<u64>,
    role_copy: Copy,
    team_copy: Copy,
    /// The stage records that counted on the issue so far, oldest first,
    /// each with the URL of its comment: the worker's history.
    verdicts: Vec<(StageRecord, String)>,
}

impl Staged {
    /// The stage issue `issue`, carrying `labels`, is at by its stage
    /// records as they stand now — or, its last stage having passed to the
    /// landing train, the join of its open PR — with the team and its roles
    /// as they stand now, each copied here unless a copy of that commit
    /// already is. An error is an issue no worker can be started on, or a
    /// team that does not load.
    pub fn enter(fleet: &Fleet, bound: &Bound, issue: u64, labels: &[String]) -> Result<Staged> {
        let kept = keep(&fleet.gh, &Copies::here()?, &bound.team)?;
        let (records, urls) = records_of(&fleet.gh, issue)?;
        let pr = closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number);
        // A landing worker is at no stage: it may commit, to resolve a
        // conflict on joining, and its workflow names no model for it.
        let (workflow, work, role, at, joins) = match due(&kept.team, labels, &records, pr) {
            Due::Worker(entry) => (entry.workflow, entry.work, entry.role, Some(entry.stage), None),
            Due::Lander(lander) => {
                (lander.workflow, lander.work, lander.role, None, Some(lander.pr))
            }
            Due::Human(why) | Due::Nothing(why) => {
                bail!("issue #{issue} cannot be started: {why}")
            }
        };
        let history = history(workflow, &records);
        let verdicts = records.into_iter().zip(urls).zip(history.readings);
        Ok(Staged {
            verdicts: verdicts
                .filter(|(_, reading)| matches!(reading, Reading::Verdict | Reading::Sent))
                .map(|(verdict, _)| verdict)
                .collect(),
            issue,
            flow: workflow.name.clone(),
            stage: at.map_or(LAND, |stage| &stage.name).to_string(),
            role: work.role.clone(),
            skill: work.skill.clone(),
            writes: at.is_none_or(|stage| stage.writes),
            model: at.and_then(|stage| stage.model.clone()),
            base_branch: fleet.config.base_branch.clone(),
            escalate_label: kept.team.escalate_label.clone(),
            lock_label: bound.lock_label.clone(),
            joins,
            role_copy: kept.role(role)?.clone(),
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
            guard: &crate::own_command("guard"),
            facts: Facts {
                issue: self.issue,
                flow: &self.flow,
                stage: &self.stage,
                role: &self.role,
                skill: &self.skill,
                writes: self.writes,
                // No consult is started yet: a stage's own worker never is one.
                consult: false,
                base_branch: &self.base_branch,
                escalate_label: &self.escalate_label,
                lock_label: &self.lock_label,
                role_commit: &self.role_copy.commit,
                team_commit: &self.team_copy.commit,
            },
        };
        in_role(launch, &start)
    }

    /// Keeps, for a stage that writes nothing, where the branch in `worktree`
    /// is as its worker starts under `claim`: what the fleet checks once the
    /// stage has declared (`advance::advance`).
    pub fn keep_start(&self, worktree: &Worktree, claim: &Claim) -> Result<()> {
        if self.writes {
            return Ok(());
        }
        worktree.keep_writes_nothing(|tip| WritesNothing {
            claim: claim.sha.clone(),
            tip,
            stage: self.stage.clone(),
        })
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

    /// Which brief the worker is given: the stage's, or the join-only one.
    pub fn prompt(&self) -> &'static str {
        if self.joins.is_some() { "join" } else { "stage" }
    }

    /// Writes the worker's brief into `worktree` → the file: its stage's, or
    /// — a landing worker's — the join-only brief of its PR.
    pub fn write_brief(&self, fleet: &Fleet, worktree: &Worktree, title: &str) -> Result<PathBuf> {
        let path = worktree.path.display().to_string();
        // A worker starts in its role's copy: its commands are run from the worktree.
        let own =
            |arguments: &str| format!("cd {} && {}", quoted(&path), crate::own_command(arguments));
        let gate = own("gate --train");
        if let Some(pr) = self.joins {
            let fields = WorkerFields {
                number: self.issue,
                title,
                branch: &worktree.branch,
                worktree: &path,
                base_branch: &fleet.config.base_branch,
                gate_command: &gate,
                land_command: &own(&format!("land --issue {}", self.issue)),
                wake_command: &own("wake"),
            };
            let landing = Landing { role: &self.role, skill: &self.skill };
            let prompt = landing_prompt(&fields, pr, landing).map_err(|e| anyhow!(e))?;
            return worktree.write_brief(&prompt);
        }
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
            worktree: &path,
            base_branch: &fleet.config.base_branch,
            precedents: &self.team_copy.dir.join(PRECEDENTS).display().to_string(),
            records: &records,
            verdict_command: &own(&verdict),
            gate_command: self.writes.then_some(gate.as_str()),
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

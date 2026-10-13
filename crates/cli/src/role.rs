//! Starting a worker in its role (docs/adr/0009): the stage a bound repo's
//! issue is dispatched to, the copies its worker starts among, and what a
//! start asks of the worker launch command.

use std::path::PathBuf;

use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::Bound;
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::{
    Landing, Said, StageFields, WorkerFields, consult_prompt, landing_prompt, stage_prompt,
};
use afkconvoy_core::role::{Due, Facts, On, Start, due};
use afkconvoy_core::stage_record::{Reading, StageRecord, history};
use afkconvoy_core::team::{BY_DEFAULT, LAND, PRECEDENTS};
use afkconvoy_core::tools::{Found, Tool, fault, refused};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::copies::{Copies, Copy};
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::launch::Launches;
use crate::orca::{Under, Worktree};
use crate::stage::records_of;
use crate::team::keep;
use crate::tools;

/// One stage of one issue, as its worker is started on it — or its consult,
/// or the join that follows the last one: the role and the team at the
/// commits fixed for it.
pub struct Staged {
    issue: u64,
    /// The workflow's name.
    flow: String,
    /// The stage's name — of a consult, the stage that asked; `land` for the join.
    stage: String,
    role: String,
    skill: String,
    writes: bool,
    /// Whether the worker is the stage's consult: it rules on the stage's
    /// `escalate`, and writes no code whatever the stage may.
    consult: bool,
    /// The harness the worker is a session of: the stage's own worker's is
    /// the one the stage names; a consult's, a landing worker's and that of
    /// a stage naming none is the default the repo's config names.
    harness: Harness,
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
    /// The team's workflows, by name: what a stage that splits its issue
    /// may take a child in to.
    workflows: Vec<String>,
    /// The stage records that counted on the issue so far, oldest first,
    /// each with the URL of its comment: the worker's history.
    verdicts: Vec<(StageRecord, String)>,
}

impl Staged {
    /// The stage issue `issue`, carrying `labels`, is at by its stage
    /// records as they stand now — or the consult its `escalate` goes to, or,
    /// its last stage having passed to the landing train, the join of its
    /// open PR — with the team and its roles
    /// as they stand now, each copied here unless a copy of that commit
    /// already is. An error is an issue no worker can be started on, or a
    /// team that does not load.
    pub fn enter(fleet: &Fleet, bound: &Bound, issue: u64, labels: &[String]) -> Result<Staged> {
        let kept = keep(&fleet.gh, &Copies::here()?, &bound.team)?;
        let (records, urls) = records_of(&fleet.gh, issue)?;
        let pr = closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number);
        // A landing worker is at no stage: it may commit, to resolve a
        // conflict on joining, and its workflow names no model for it.
        let due = due(&kept.team, labels, &records, pr);
        let consult = matches!(due, Due::Consult(_));
        let (workflow, work, role, at, joins) = match due {
            Due::Worker(entry) | Due::Consult(entry) => {
                (entry.workflow, entry.work, entry.role, Some(entry.stage), None)
            }
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
                .filter(|(_, reading)| {
                    matches!(reading, Reading::Verdict | Reading::Sent | Reading::LockClosed)
                })
                .map(|(verdict, _)| verdict)
                .collect(),
            issue,
            flow: workflow.name.clone(),
            stage: at.map_or(LAND, |stage| &stage.name).to_string(),
            role: role.name.clone(),
            skill: work.skill.clone(),
            writes: at.is_none_or(|stage| stage.writes),
            consult,
            // The stage's harness and its model are its own worker's: a
            // workflow names none for a consult.
            harness: at
                .filter(|_| !consult)
                .and_then(|stage| stage.harness(bound.harness))
                .unwrap_or(bound.harness),
            model: at.filter(|_| !consult).and_then(|stage| stage.model.clone()),
            base_branch: fleet.config.base_branch.clone(),
            escalate_label: kept.team.escalate_label.clone(),
            lock_label: fleet.config.lock_label.clone(),
            joins,
            role_copy: kept.role(role)?,
            team_copy: kept.copy.clone(),
            workflows: kept.team.workflows.iter().map(|workflow| workflow.name.clone()).collect(),
        })
    }

    /// The harness the worker is a session of.
    pub fn harness(&self) -> Harness {
        self.harness
    }

    /// The line the terminal on `worktree` runs to start the worker with
    /// `launch`, the worker launch command of its harness. What this host
    /// keeps for that harness is put in place first (`kept::keep`).
    pub fn command(&self, launch: &str, worktree: &Worktree) -> Result<String> {
        let start = Start {
            role_dir: &self.role_copy.dir.display().to_string(),
            worktree: &worktree.path.display().to_string(),
            team_dir: &self.team_copy.dir.display().to_string(),
            model: self.model.as_deref(),
            guard: &crate::own_command().fixed(" guard").own(),
            kept: &crate::kept::keep(self.harness)?.display().to_string(),
            facts: Facts {
                on: On::Stage { issue: self.issue, flow: &self.flow, stage: &self.stage },
                role: &self.role,
                skill: &self.skill,
                writes: self.writes,
                consult: self.consult,
                base_branch: &self.base_branch,
                escalate_label: &self.escalate_label,
                lock_label: &self.lock_label,
                role_commit: &self.role_copy.commit,
                team_commit: &self.team_copy.commit,
            },
        };
        Ok(self.harness.in_role(launch, &start))
    }

    /// Keeps, for a stage that writes nothing and for a consult, where the
    /// branch in `worktree` is as its worker starts under `claim`: what the
    /// fleet checks once it has declared (`advance::advance`).
    /// Every start also keeps the commits it is made under (`orca::Under`).
    pub fn keep_start(&self, worktree: &Worktree, claim: &Claim) -> Result<()> {
        worktree.keep_under(&Under {
            stage: self.stage.clone(),
            role: self.role.clone(),
            role_commit: self.role_copy.commit.clone(),
            team_commit: self.team_copy.commit.clone(),
        })?;
        if self.writes && !self.consult {
            return Ok(());
        }
        worktree.keep_writes_nothing(|tip| WritesNothing {
            claim: claim.id.clone(),
            tip,
            stage: self.stage.clone(),
            consult: self.consult,
        })
    }

    /// Refuses a start under `claim` when a verdict was already declared
    /// under it: a claim declares once, so the stage started would have no
    /// say. The tick ends such a claim, and the next start takes another.
    pub fn refuse_spent(&self, claim: Option<&Claim>) -> Result<()> {
        let spent = |claim: &&Claim| {
            self.verdicts.iter().any(|(said, _)| said.claim() == Some(claim.id.as_str()))
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

    /// Which brief the worker is given: the stage's, the consult's, or the
    /// join-only one.
    pub fn prompt(&self) -> &'static str {
        match (self.joins, self.consult) {
            (Some(_), _) => "join",
            (None, true) => "consult",
            (None, false) => "stage",
        }
    }

    /// Writes the worker's brief into `worktree` → the file: its stage's, a
    /// consult's of the `escalate` it rules on, or — a landing worker's — the
    /// join-only brief of its PR.
    pub fn write_brief(&self, fleet: &Fleet, worktree: &Worktree, title: &str) -> Result<PathBuf> {
        let path = worktree.path.display().to_string();
        // A worker starts in its role's copy: its commands are run from the worktree.
        let own = || crate::own_command_in(&path);
        let gate = own().fixed(" gate --train").own();
        if let Some(pr) = self.joins {
            let fields = WorkerFields {
                number: self.issue,
                title,
                branch: &worktree.branch,
                worktree: &path,
                base_branch: &fleet.config.base_branch,
                gate_command: &gate,
                land_command: &own().fixed(" land --issue ").number(self.issue).own(),
                wake_command: &own().fixed(" wake").own(),
            };
            let landing = Landing { role: &self.role, skill: &self.skill };
            let prompt = landing_prompt(&fields, pr, landing).map_err(|e| anyhow!(e))?;
            return worktree.write_brief(&prompt);
        }
        // What a brief tells a worker to declare with: this tool's `subcommand`,
        // of the worker's issue and stage.
        let declare = |subcommand: &'static str| {
            let line = own().fixed(subcommand).fixed(" --issue ").number(self.issue);
            line.fixed(" --stage ").word(&self.stage).own()
        };
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
            verdict_command: &declare(" verdict"),
            intake_command: &own().fixed(" intake").own(),
            workflows: &self.workflows,
            gate_command: self.writes.then_some(gate.as_str()),
        };
        let prompt = if self.consult {
            consult_prompt(&fields, &declare(" rule"))
        } else {
            stage_prompt(&fields)
        };
        worktree.write_brief(&prompt.map_err(|e| anyhow!(e))?)
    }

    pub fn json(&self) -> Value {
        json!({"flow": self.flow, "name": self.stage, "role": self.role, "skill": self.skill,
            "writes": self.writes, "consult": self.consult, "harness": self.harness,
            "model": self.model,
            "role_commit": self.role_copy.commit, "role_dir": self.role_copy.dir,
            "team_commit": self.team_copy.commit, "team_dir": self.team_copy.dir})
    }
}

/// The worker launch command of one harness, as far as this host can say
/// what it is and whether it can start that harness's workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub harness: Harness,
    /// What calls for the harness (`Team::harnesses`); nothing in an unbound
    /// repo, which knows of no harness.
    pub callers: Vec<String>,
    pub command: String,
    /// What this host has of the harness, when the command runs it by its
    /// own name (`tools::harness`). `None` also of the default harness: it
    /// is among what every start looks for (`tools::look`).
    pub tool: Option<(Tool, Found)>,
    /// Why it cannot start a worker, when it cannot.
    pub fault: Option<String>,
    /// Why it does not take what starts a worker in its role, when it does
    /// not. Asked in a bound repo only.
    pub flags_fault: Option<String>,
}

/// Every harness the fleet of a repo `bound` to a team could be asked for,
/// each with what calls for it (`Team::harnesses`): the repo's default, and
/// whatever the team's workflows and the patrols the repo runs name, as the
/// team stands now. A team that cannot be read or does not load calls for
/// the default alone: no worker is started under it, and each tick says why.
pub fn called_for(bound: &Bound) -> Vec<(Harness, Vec<String>)> {
    let gh = GitHub { dir: PathBuf::from(".") };
    match Copies::here().and_then(|copies| keep(&gh, &copies, &bound.team)) {
        Ok(kept) => kept.team.harnesses(bound.harness, &bound.patrols),
        Err(_) => vec![(bound.harness, vec![BY_DEFAULT.to_string()])],
    }
}

/// Checks the worker launch command of each harness `called` for, as
/// `launches` has it, asking the login shell a worker gets: that its first
/// word resolves there, that a harness it runs by name holds credentials,
/// and — in a bound repo — that it takes what the fleet adds to it for that
/// harness. Each fault is said with its harness and what
/// calls for it. A command only a human could say, in a start that asks
/// none, is left out.
pub fn check(launches: &Launches, called: &[(Harness, Vec<String>)]) -> Result<Vec<Checked>> {
    let mut checked = vec![];
    for (harness, callers) in called {
        let Some(proposed) = launches.of(*harness)? else { continue };
        let said = |fault: String| launches.said(&fault, *harness, callers);
        let takes = || harness.takes(proposed.as_str(), proposed.flags().as_deref());
        checked.push(Checked {
            harness: *harness,
            callers: callers.clone(),
            command: proposed.as_str().to_string(),
            tool: tools::harness(proposed).filter(|_| *harness != launches.default()),
            fault: proposed.fault().map(said),
            flags_fault: takes().err().filter(|_| launches.bound()).map(said),
        });
    }
    Ok(checked)
}

impl Checked {
    /// Whether the harness itself is not on this host: nothing else is then
    /// to be said of its command.
    pub fn missing(&self) -> bool {
        matches!(self.tool, Some((_, Found::Missing)))
    }
}

/// Checks that every harness the fleet of a repo `bound` to a team could be
/// asked for can be started on this host (`check`). An error names every
/// fault at once — a harness that is missing or holds no credentials with
/// what calls for it and the line that fixes it: none is started in
/// another's place, so a start that cannot start them all starts no worker.
pub fn probe(launches: &Launches, bound: &Bound) -> Result<()> {
    let (mut lacking, mut faults) = (vec![], vec![]);
    for checked in check(launches, &called_for(bound))? {
        let missing = checked.missing();
        if let Some((tool, found)) = &checked.tool {
            lacking.extend(fault(*tool, found, None, &checked.callers));
        }
        if !missing {
            faults.extend([checked.fault, checked.flags_fault].into_iter().flatten());
        }
    }
    let faults = (!faults.is_empty()).then(|| faults.join("\n"));
    match refused(&lacking, faults.as_deref()) {
        Some(why) => bail!(why),
        None => Ok(()),
    }
}

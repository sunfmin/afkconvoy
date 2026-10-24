//! Starting a worker in its role (docs/adr/0009): the stage a bound repo's
//! issue is dispatched to, the copies its worker starts among, and what a
//! start asks of the worker launch command.

use std::path::{Path, PathBuf};

use afkconvoy_core::casting;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::{Bound, RepoLabels};
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::position::{ConsultOn, Position, Reason};
use afkconvoy_core::prompt::{
    Consulting, Gate, Landing, Said, StageFields, WorkerFields, consult_prompt, landing_prompt,
    stage_prompt,
};
use afkconvoy_core::role::{Due, Facts, On, Rules, Start, Told, due};
use afkconvoy_core::stage_record::{History, Reading, StageRecord, Stated, history};
use afkconvoy_core::stock;
use afkconvoy_core::team::{LAND, PRECEDENTS, Team};
use afkconvoy_core::tools::{Found, Tool, fault, refused};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::copies::{Copies, Copy};
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::launch::Launches;
use crate::orca::{Under, Worktree};
use crate::stage::records_of;
use crate::team::keep;
use crate::tools;
use crate::topic::See;

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
    /// The harness the worker is a session of: what its role's range makes
    /// of the issue's casting (`Fleet::cast_in`), or the stock consult's.
    harness: Harness,
    model: Option<String>,
    /// The name the issue was given that matches nothing held, with what
    /// the worker runs instead.
    unmatched: Option<(String, casting::Cast)>,
    /// What the guard holds every worker to: the repo's base branch, the
    /// team's escalate label and the repo's lock label.
    base_branch: String,
    escalate_label: String,
    lock_label: String,
    /// The PR a landing worker gates and joins. `None` at a stage.
    joins: Option<u64>,
    role_copy: Copy,
    team_copy: Copy,
    /// The team rules, in the team's copy, of a team that has them.
    team_rules: Option<String>,
    /// Whether the worker is given the host user's skills and rules: its
    /// role's say (`Role::host_user`), or the team's for the stock consult.
    host_user: bool,
    dsh_profile: afkconvoy_core::team::DshProfile,
    /// Whether the team offers splitting in stage briefs.
    split: bool,
    /// The team's workflows, by name: what a stage that splits its issue
    /// may take a child in to.
    workflows: Vec<String>,
    /// The stage records that counted on the issue so far, oldest first,
    /// each with the URL of its comment: the worker's history.
    verdicts: Vec<(StageRecord, String)>,
    /// Of a consult asked of a hand-off, what a human handed the issue would
    /// be told (`handed_said`).
    handed: Option<String>,
}

/// What a human handed the issue would be told, where `history` — read from
/// `records`, each with the URL of its comment — has it with its stage's
/// consult for a hand-off: the reason and the evidence of the failure the
/// fleet recorded, or why its records hand it on, with every round of the
/// rework a breaker stopped. `None` of an `escalate`, and anywhere else.
fn handed_said(history: &History, records: &[StageRecord], urls: &[String]) -> Option<String> {
    let Position::Consult(stage, _, ConsultOn::HandOff(reason)) = history.place.position else {
        return None;
    };
    if let Reason::Failed(_) = reason {
        let recorded =
            records.iter().rev().find(|record| matches!(record.stated, Stated::Failed(_)));
        return recorded.map(|record| record.text.clone());
    }
    let reason = reason.said(stage);
    let mut chars = reason.chars();
    let mut said: String =
        chars.next().into_iter().flat_map(char::to_uppercase).chain(chars).collect();
    said.push('.');
    if let Some(rounds) = history.rounds_said(records, urls) {
        said.push_str(&format!("\n\n{rounds}"));
    }
    Some(said)
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
        let kept = keep(&fleet.gh, &Copies::of_host()?, &bound.team, &fleet.config.labels())?;
        let (records, urls) = records_of(&fleet.gh, issue)?;
        let pr = closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number);
        // A landing worker is at no stage: it may commit, to resolve a
        // conflict on joining, and its workflow names no model for it.
        let due = due(&kept.team, labels, &records, pr);
        let consult = matches!(due, Due::Consult(_) | Due::Stock(..));
        // Who the worker is: a skill of a role, or the stock consult, which
        // is no role of the team.
        let (workflow, by, at, joins) = match due {
            Due::Worker(entry) | Due::Consult(entry) => {
                (entry.workflow, Some((entry.work, entry.role)), Some(entry.stage), None)
            }
            Due::Stock(workflow, stage) => (workflow, None, Some(stage), None),
            Due::Lander(lander) => {
                (lander.workflow, Some((lander.work, lander.role)), None, Some(lander.pr))
            }
            // What waits for a human is answered by one; anything else is
            // where the issue is in its workflow.
            Due::Human(why) => {
                let why = format!("issue #{issue} cannot be started: {why}");
                return Err(See::Humans.refusal(why));
            }
            Due::Nothing(why) => {
                let why = format!("issue #{issue} cannot be started: {why}");
                return Err(See::Workflows.refusal(why));
            }
        };
        let history = history(workflow, &records);
        let handed = handed_said(&history, &records, &urls);
        let verdicts = records.into_iter().zip(urls).zip(history.readings);
        // A caster's casting to a paused harness is cast again: until it is,
        // the issue runs as one with no casting.
        let labels = match fleet.recast(issue, labels)? {
            true => casting::without_casting(labels),
            false => labels.to_vec(),
        };
        let kit = crate::kit::running()?.0.range();
        let ranges = casting::stage_ranges(&kept.team, workflow, &kit);
        // A worker in a role runs the issue's casting within its role's
        // range — its range's first where it matches nothing held
        // (docs/adr/0020); the stock consult runs what the host kit names for
        // it. This host's chosen harness, where a switch chose one, wins over
        // both.
        let cast = match &by {
            Some((_, role)) => {
                casting::unmatched_in(fleet.cast_in(role, &labels)?, &labels, &ranges)
            }
            None => fleet.stock_consult()?,
        };
        let (harness, model) = (cast.harness, cast.model.clone());
        let (role, skill, role_copy, host_user, dsh_profile) = match by {
            Some((work, role)) => (
                role.name.clone(),
                work.skill.clone(),
                kept.role(role, harness)?,
                role.host_user,
                role.dsh_profile,
            ),
            None => (
                stock::ROLE.to_string(),
                stock::SKILL.to_string(),
                Copies::of_host()?.stock()?,
                kept.team.host_user,
                kept.team.dsh_profile,
            ),
        };
        Ok(Staged {
            verdicts: verdicts
                .filter(|(_, reading)| {
                    matches!(
                        reading,
                        Reading::Verdict | Reading::Sent | Reading::LockClosed | Reading::Failed
                    )
                })
                .map(|(verdict, _)| verdict)
                .collect(),
            issue,
            handed,
            flow: workflow.name.clone(),
            stage: at.map_or(LAND, |stage| &stage.name).to_string(),
            role,
            skill,
            writes: at.is_none_or(|stage| stage.writes),
            consult,
            harness,
            model,
            unmatched: cast.unmatched.clone().map(|name| (name, cast)),
            base_branch: fleet.config.base_branch.clone(),
            escalate_label: kept.team.escalate_label.clone(),
            lock_label: fleet.config.lock_label.clone(),
            joins,
            role_copy,
            team_copy: kept.copy.clone(),
            team_rules: kept.rules(),
            host_user,
            dsh_profile,
            split: kept.team.split,
            workflows: kept.team.workflows.iter().map(|workflow| workflow.name.clone()).collect(),
        })
    }

    /// The harness the worker is a session of.
    pub fn harness(&self) -> Harness {
        self.harness
    }

    /// The name the issue was given that matches nothing held, with what
    /// the worker runs instead: what the issue is told once.
    pub fn unmatched(&self) -> Option<&(String, casting::Cast)> {
        self.unmatched.as_ref()
    }

    /// The line the terminal on `worktree` runs to start the worker with
    /// `launch`, the worker launch command of its harness. What this host
    /// keeps for that harness is put in place first (`kept::keep`).
    pub fn command(&self, launch: &str, worktree: &Worktree) -> Result<String> {
        let told = ToldRules::read(self.harness, &worktree.path, self.team_rules.as_deref())?;
        let (kept, shadowed) =
            crate::kept::for_worker(self.harness, self.host_user, &self.role_copy.dir)?;
        let start = Start {
            role_dir: &self.role_copy.dir.display().to_string(),
            worktree: &worktree.path.display().to_string(),
            team_dir: &self.team_copy.dir.display().to_string(),
            model: self.model.as_deref(),
            guard: &crate::own_command().fixed(" guard").own(),
            kept: &kept,
            host_user: self.host_user,
            dsh_profile: self.dsh_profile,
            shadowed: &shadowed,
            told: told.told(),
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
        // Another checkout is gated from inside it.
        let gate_alone = crate::own_command().fixed(" gate").own();
        if let Some(pr) = self.joins {
            let fields = WorkerFields {
                number: self.issue,
                title,
                branch: &worktree.branch,
                worktree: &path,
                base_branch: &fleet.config.base_branch,
                gate_command: &gate,
                gate_alone_command: &gate_alone,
                ready_label: &fleet.config.ready_label,
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
            team_rules: self.team_rules.as_deref(),
            records: &records,
            verdict_command: &declare(" verdict"),
            intake_command: &own().fixed(" intake").own(),
            split: self.split,
            workflows: &self.workflows,
            gate: self.writes.then_some(Gate { train: &gate, alone: &gate_alone }),
        };
        let prompt = if self.consult {
            let issue = own().fixed(" show --issue ").number(self.issue).own();
            let consulting = Consulting {
                rule: &declare(" rule"),
                handed: self.handed.as_deref(),
                show: &issue,
                gate: &own().fixed(" gate").own(),
                doctor: &own().fixed(" doctor").own(),
                plan: &own().fixed(" plan").own(),
            };
            consult_prompt(&fields, &consulting)
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

/// The rules a worker is told on the line that starts it, as they stand
/// when it starts: each with the file it was read from.
#[derive(Default)]
pub struct ToldRules {
    repo: Option<(String, String)>,
    team: Option<(String, String)>,
}

impl ToldRules {
    /// Reads what a worker of `harness` is told: the repo's own rules, from
    /// the first file of `Harness::told` that `worktree` holds, and the team
    /// rules, from `team_rules`, of a team that has them. Nothing is read
    /// for a harness that loads both itself.
    pub fn read(harness: Harness, worktree: &Path, team_rules: Option<&str>) -> Result<ToldRules> {
        let names = harness.told();
        if names.is_empty() {
            return Ok(ToldRules::default());
        }
        let read = |file: PathBuf| {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read the rules in {}", file.display()))?;
            anyhow::Ok((file.display().to_string(), text))
        };
        let held = names.iter().map(|name| worktree.join(name)).find(|file| file.is_file());
        Ok(ToldRules {
            repo: held.map(read).transpose()?,
            team: team_rules.map(PathBuf::from).map(read).transpose()?,
        })
    }

    /// What was read, as a start says it.
    pub fn told(&self) -> Told<'_> {
        fn rules(read: &(String, String)) -> Rules<'_> {
            Rules { file: &read.0, text: &read.1 }
        }
        Told { repo: self.repo.as_ref().map(rules), team: self.team.as_ref().map(rules) }
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

/// What calls for the host kit's first: every worker in no role but the
/// stock consult, and whatever nothing casts.
pub const BY_FIRST: &str = "the host kit's first";

/// Adds `by` to what calls for `harness` in `called`.
fn call(called: &mut Vec<(Harness, Vec<String>)>, harness: Harness, by: String) {
    match called.iter_mut().find(|(named, _)| *named == harness) {
        Some((_, callers)) => callers.push(by),
        None => called.push((harness, vec![by])),
    }
}

/// The team the repo of the checkout `repo` is `bound` to as it stands now,
/// read against the repo's `labels`: `None` where it cannot be read or does
/// not load — no worker is started under it, and each tick says why.
pub fn current(repo: &Repo, bound: &Bound, labels: &RepoLabels) -> Option<Team> {
    let gh = GitHub { dir: repo.dir.clone() };
    let kept = Copies::of_host().and_then(|copies| keep(&gh, &copies, &bound.team, labels));
    kept.ok().map(|kept| kept.team)
}

/// Every harness the fleet of the checkout `repo`, bound to `team`, could be
/// asked for, each with what calls for it: the host kit's first, every
/// harness of each role's range (`casting::range`), and the stock consult's,
/// as the host kit names it. No team (`current`) calls for the first alone.
pub fn called_for(repo: &Repo, team: Option<&Team>) -> Vec<(Harness, Vec<String>)> {
    // A harness this host's switch chose is the one every worker starts on.
    let chosen = crate::switch::chosen(repo).ok();
    if let Some(chosen) = chosen.flatten() {
        return vec![(chosen, vec![crate::switch::CHOSEN_BY.to_string()])];
    }
    let Ok(kit) = crate::kit::load() else { return vec![] };
    let mut called = vec![];
    if let Some(first) = kit.first() {
        call(&mut called, first.harness, BY_FIRST.to_string());
    }
    if let Some(team) = team {
        for role in &team.roles {
            for held in casting::range(role, &kit.range()) {
                call(&mut called, held.harness, format!("the range of the role `{}`", role.name));
            }
        }
    }
    // The stock consult runs what the host kit names for it.
    if let Some(stock) = kit.stock_consult() {
        call(&mut called, stock.harness, BY_STOCK_CONSULT.to_string());
    }
    called
}

/// What calls for the harness the host kit names for the stock consult.
const BY_STOCK_CONSULT: &str = "the host kit's stock consult";

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
        let proposed = match launches.of(*harness) {
            Ok(Some(proposed)) => proposed,
            Ok(None) => continue,
            Err(error) => {
                checked.push(Checked {
                    harness: *harness,
                    callers: callers.clone(),
                    command: String::new(),
                    tool: None,
                    fault: Some(launches.said(&format!("{error:#}"), *harness, callers)),
                    flags_fault: None,
                });
                continue;
            }
        };

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

/// Checks that every harness the fleet of the checkout `repo`, `bound` to a
/// team, could be asked for can be started on this host (`check`). An error names every
/// fault at once — a harness that is missing or holds no credentials with
/// what calls for it and the line that fixes it: none is started in
/// another's place, so a start that cannot start them all starts no worker.
pub fn probe(repo: &Repo, launches: &Launches, bound: &Bound, labels: &RepoLabels) -> Result<()> {
    let team = current(repo, bound, labels);
    if let Some(why) = refusal(check(launches, &called_for(repo, team.as_ref()))?) {
        bail!(why);
    }
    // A role whose range holds nothing refuses no start: its stages wait
    // (`casting::nothing_safe`), and nothing else does.
    Ok(())
}

/// Checks that a worker of `harness` can be started on this host, for what
/// `caller` says calls for it, as `probe` checks each harness — and whether
/// it holds credentials even where it is the default harness, which a start
/// looks for apart. An error names the harness and every fault.
pub fn startable(launches: &Launches, harness: Harness, caller: &str) -> Result<()> {
    let mut checked = check(launches, &[(harness, vec![caller.to_string()])])?;
    if checked.is_empty() {
        bail!("the harness `{}` has no worker launch command known here", harness.key());
    }
    for checked in &mut checked {
        if checked.tool.is_none() {
            checked.tool = launches.of(harness)?.and_then(tools::harness);
        }
    }
    match refusal(checked) {
        Some(why) => bail!("the harness `{}` cannot be started here: {why}", harness.key()),
        None => Ok(()),
    }
}

/// Why the harnesses `checked` cannot all be started, when they cannot.
fn refusal(checked: Vec<Checked>) -> Option<String> {
    let (mut lacking, mut faults) = (vec![], vec![]);
    for checked in checked {
        let missing = checked.missing();
        if let Some((tool, found)) = &checked.tool {
            lacking.extend(fault(*tool, found, None, &checked.callers));
        }
        if !missing {
            faults.extend([checked.fault, checked.flags_fault].into_iter().flatten());
        }
    }
    let faults = (!faults.is_empty()).then(|| faults.join("\n"));
    refused(&lacking, faults.as_deref())
}

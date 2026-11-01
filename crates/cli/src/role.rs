//! Starting a worker in its role (docs/adr/0009): the stage a bound repo's
//! issue is dispatched to, the copies its worker starts among, and what a
//! start asks of the worker launch command.

use std::path::{Path, PathBuf};

use afkconvoy_core::casting;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::{Bound, Config, RepoLabels};
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::position::{ConsultOn, Position, Reason};
use afkconvoy_core::prompt::{
    Consulting, Gate, Landing, Said, StageFields, WorkerFields, consult_prompt, landing_prompt,
    stage_prompt,
};
use afkconvoy_core::role::{Due, Facts, On, Rules, Start, Told, due};
use afkconvoy_core::selection::Selection;
use afkconvoy_core::stage_record::{History, Reading, StageRecord, Stated, history};
use afkconvoy_core::team::{LAND, Team};
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
    /// What the worker runs: its worker selection (`Fleet::select`).
    selection: Selection,
    /// Every kit candidate before `harness` that its range excluded.
    skipped: Vec<Value>,
    /// What the guard holds every worker to: the repo's base branch, the
    /// team's escalate label and the repo's lock label.
    base_branch: String,
    repo_base_branch: String,
    escalate_label: String,
    lock_label: String,
    /// The PR a landing worker gates and joins. `None` at a stage.
    joins: Option<u64>,
    role_copy: Copy,
    team_copy: Copy,
    /// The team rules, in the team's copy, of a team that has them.
    team_rules: Option<String>,
    /// Whether the worker is given the host user's skills and rules: its
    /// role's say (`Role::host_user`).
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

/// What a human handed the issue would be told, where `history` — its
/// records each with the URL of its comment in `urls` — has it with its stage's
/// consult for a hand-off: the reason and the evidence of the failure the
/// fleet recorded, or why its records hand it on, with every round of the
/// rework a breaker stopped. `None` of an `escalate`, and anywhere else.
fn handed_said(history: &History, urls: &[String]) -> Option<String> {
    let Position::Consult(stage, _, ConsultOn::HandOff(reason)) = history.place.position else {
        return None;
    };
    if let Reason::Failed(_) = reason {
        let mut records = history.records.iter().rev();
        let recorded = records.find(|record| matches!(record.stated, Stated::Failed(_)));
        return recorded.map(|record| record.text.clone());
    }
    let reason = reason.said(stage);
    let mut chars = reason.chars();
    let mut said: String =
        chars.next().into_iter().flat_map(char::to_uppercase).chain(chars).collect();
    said.push('.');
    if let Some(rounds) = history.rounds_said(urls) {
        said.push_str(&format!("\n\n{rounds}"));
    }
    Some(said)
}

impl Staged {
    /// The role this worker performs, including a consult's own role.
    pub fn role(&self) -> &str {
        &self.role
    }

    pub fn is_consult(&self) -> bool {
        self.consult
    }
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
        let consult = matches!(due, Due::Consult(_));
        // Who the worker is: a skill of a role named by the team.
        let (workflow, by, at, joins) = match due {
            Due::Worker(entry) | Due::Consult(entry) => {
                (entry.workflow, (entry.work, entry.role), Some(entry.stage), None)
            }
            Due::Lander(lander) => {
                (lander.workflow, (lander.work, lander.role), None, Some(lander.pr))
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
        let history = history(workflow, records);
        let handed = handed_said(&history, &urls);
        let verdicts = history.records.into_iter().zip(urls).zip(history.readings);
        let labels = labels.to_vec();
        // A worker in a role runs the issue's one model label where that
        // harness holds it, or that harness's user default (docs/adr/0020).
        // A checkout that names a harness runs that harness.
        let (work, role) = by;
        let selection = fleet.select_in(Some(role), &labels, Some(issue))?;
        let harness = selection.harness();
        let (role, skill, role_copy, host_user, dsh_profile) = (
            role.name.clone(),
            work.skill.clone(),
            kept.role(role, harness)?,
            role.host_user,
            role.dsh_profile,
        );
        let skipped = fleet.skipped_before(harness, Some(by.1), Some(issue));
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
            selection,
            skipped,
            base_branch: fleet.task_branch(issue)?,
            repo_base_branch: fleet.config.base_branch.clone(),
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
        self.selection.harness()
    }

    /// What the worker runs: its worker selection.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The stage this worker performs. A landing worker's is `land`.
    pub fn stage(&self) -> &str {
        &self.stage
    }

    /// The line the terminal on `worktree` runs to start the worker with
    /// `launch`, the worker launch command of its harness. What this host
    /// keeps for that harness is put in place first (`kept::keep`).
    pub fn command(
        &self,
        launch: &str,
        worktree: &Worktree,
        model: Option<&str>,
    ) -> Result<String> {
        let told = ToldRules::read(
            self.harness(),
            &worktree.path,
            self.team_rules.as_deref(),
            &self.role_copy.dir,
        )?;
        let (kept, shadowed) = crate::kept::for_worker(
            self.harness(),
            self.host_user,
            &self.role_copy.dir,
            &worktree.path,
            &told.told(),
            &crate::own_command().fixed(" guard").own(),
        )?;
        let start = Start {
            role_dir: &self.role_copy.dir.display().to_string(),
            worktree: &worktree.path.display().to_string(),
            team_dir: &self.team_copy.dir.display().to_string(),
            model,
            guard: &crate::own_command().fixed(" guard").own(),
            kept: &kept,
            host_user: self.host_user,
            dsh_profile: self.dsh_profile,
            host_rules: &crate::kept::host_rules(self.harness()),
            shadowed: &shadowed,
            told: told.told(),
            facts: Facts {
                on: On::Stage { issue: self.issue, flow: &self.flow, stage: &self.stage },
                role: &self.role,
                skill: &self.skill,
                writes: self.writes,
                consult: self.consult,
                base_branch: &self.repo_base_branch,
                target_branch: (self.base_branch != self.repo_base_branch)
                    .then_some(self.base_branch.as_str()),
                escalate_label: &self.escalate_label,
                lock_label: &self.lock_label,
                role_commit: &self.role_copy.commit,
                team_commit: &self.team_copy.commit,
            },
        };
        Ok(self.harness().in_role(launch, &start))
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
        let gate = own().fixed(" gate --land").own();
        // Another checkout is gated from inside it.
        let gate_alone = crate::own_command().fixed(" gate").own();
        let fast_gate = fleet.config.gates.as_ref().map(|_| own().fixed(" gate --fast").own());
        if let Some(pr) = self.joins {
            let fields = WorkerFields {
                number: self.issue,
                tracker: &fleet.gh.tracker,
                title,
                branch: &worktree.branch,
                worktree: &path,
                base_branch: &self.base_branch,
                gate_command: &gate,
                gate_alone_command: &gate_alone,
                lock_command: &own().fixed(" lock").own(),
                ready_label: &fleet.config.ready_label,
                land_command: &own().fixed(" land --issue ").number(self.issue).own(),
                wake_command: &own().fixed(" wake").own(),
                fast_gate_command: fast_gate.as_deref(),
            };
            let landing = Landing { role: &self.role, skill: &self.skill };
            let mut prompt = landing_prompt(&fields, pr, landing).map_err(|e| anyhow!(e))?;
            prompt.push_str(&crate::delivery::brief(&fleet.repo, self.issue)?);
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
        let stage_gate = own().fixed(" gate").own();
        let fields = StageFields {
            number: self.issue,
            tracker: &fleet.gh.tracker,
            title,
            flow: &self.flow,
            stage: &self.stage,
            role: &self.role,
            skill: &self.skill,
            branch: &worktree.branch,
            worktree: &path,
            base_branch: &self.base_branch,
            team_rules: self.team_rules.as_deref(),
            records: &records,
            verdict_command: &declare(" verdict"),
            lock_command: &own().fixed(" lock").own(),
            intake_command: &own().fixed(" intake").own(),
            split: self.split,
            workflows: &self.workflows,
            gate: self.writes.then_some(Gate { standalone: &stage_gate, alone: &gate_alone }),
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
        let mut prompt = prompt.map_err(|e| anyhow!(e))?;
        if self.consult {
            let unworked = crate::blockers::for_issue(&fleet.repo, &fleet.gh, self.issue)?;
            if !unworked.is_empty() {
                prompt.push_str(&format!(
                    "\n## Blockers no fleet works\n\n{}\n",
                    crate::blockers::evidence(&unworked)
                ));
            }
        }
        prompt.push_str(&crate::delivery::brief(&fleet.repo, self.issue)?);
        worktree.write_brief(&prompt)
    }

    pub fn json(&self) -> Value {
        json!({"flow": self.flow, "name": self.stage, "role": self.role, "skill": self.skill,
            "writes": self.writes, "consult": self.consult, "harness": self.harness(),
            "model": self.selection.model, "skipped": self.skipped,
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
    role: Option<(String, String)>,
}

impl ToldRules {
    /// Reads what a worker of `harness` is told: the repo's own rules, from
    /// the first file of `Harness::told` that `worktree` holds, and the team
    /// rules, from `team_rules`, of a team that has them. Nothing of those is
    /// read for a harness that loads both itself. The role's rules are the
    /// file `Harness::role_file` names in `role_dir`, when that harness names
    /// one and the file is there. A harness that names none is told no role
    /// file by this read.
    pub fn read(
        harness: Harness,
        worktree: &Path,
        team_rules: Option<&str>,
        role_dir: &Path,
    ) -> Result<ToldRules> {
        let read = |file: PathBuf| {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read the rules in {}", file.display()))?;
            anyhow::Ok((file.display().to_string(), text))
        };
        let names = harness.told();
        let (repo, team) = if names.is_empty() {
            (None, None)
        } else {
            let held = names.iter().map(|name| worktree.join(name)).find(|file| file.is_file());
            (held.map(read).transpose()?, team_rules.map(PathBuf::from).map(read).transpose()?)
        };
        let role = match harness.role_file() {
            Some(name) => {
                let file = role_dir.join(name);
                if file.is_file() { Some(read(file)?) } else { None }
            }
            None => None,
        };
        Ok(ToldRules { repo, team, role })
    }

    /// What was read, as a start says it.
    pub fn told(&self) -> Told<'_> {
        fn rules(read: &(String, String)) -> Rules<'_> {
            Rules { file: &read.0, text: &read.1 }
        }
        Told {
            repo: self.repo.as_ref().map(rules),
            team: self.team.as_ref().map(rules),
            role: self.role.as_ref().map(rules),
        }
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
/// those that an issue casts, and whatever nothing casts.
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
    let gh = GitHub::of(repo).ok()?;
    let kept = Copies::of_host().and_then(|copies| keep(&gh, &copies, &bound.team, labels));
    kept.ok().map(|kept| kept.team)
}

/// Every harness `doctor` and a startup probe say something of, for the
/// checkout `repo` under `config`. `default` is what an unbound checkout
/// with no named harness is asked about, and nothing calls for it.
pub fn called_on(
    repo: &Repo,
    config: Option<&Config>,
    default: Harness,
) -> Vec<(Harness, Vec<String>)> {
    if let Some(config) = config
        && let Some(bound) = config.bound.as_ref()
    {
        let team = current(repo, bound, &config.labels());
        return called_for(repo, team.as_ref());
    }
    match crate::named_harness::named(repo) {
        Ok(Some(pinned)) => {
            vec![(pinned, vec![format!("the harness {} this checkout names", pinned.key())])]
        }
        _ => vec![(default, vec![])],
    }
}

/// Every harness the fleet of the checkout `repo`, bound to `team`, could be
/// asked for, each with what calls for it: the host kit's first, every
/// harness of each role's range (`casting::range`). No team (`current`) calls for the first alone.
pub fn called_for(repo: &Repo, team: Option<&Team>) -> Vec<(Harness, Vec<String>)> {
    if let Ok(Some(pinned)) = crate::named_harness::named(repo) {
        return vec![(pinned, vec![format!("the harness {} this checkout names", pinned.key())])];
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
    called
}

/// Checks the worker launch command of each harness `called` for, as
/// `launches` has it, asking the login shell a worker gets: that its first
/// word resolves there, that a harness it runs by name holds credentials,
/// and — where launch flags are added — that it takes what the fleet adds for that
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
        // The default harness is asked its flags, and that question is its one
        // start while a doctor budget stands. Another harness is asked its
        // login first; its flags are asked only when that start has not
        // happened. A tick has no budget, and asks both.
        let tool = if *harness == launches.default() { None } else { tools::harness(&proposed) };
        let flags_fault = if launches.adds_flags(*harness) && proposed.resolves() {
            match crate::launch::once_per_program(harness.program(), "flags", || {
                match proposed.flags() {
                    Some(printed) => (true, printed),
                    None => (false, String::new()),
                }
            }) {
                crate::launch::Once::Already => None,
                crate::launch::Once::Ran(ok, printed) => {
                    harness.takes(proposed.as_str(), ok.then_some(printed.as_str())).err().map(said)
                }
            }
        } else {
            None
        };
        checked.push(Checked {
            harness: *harness,
            callers: callers.clone(),
            command: proposed.as_str().to_string(),
            tool,
            fault: proposed.fault().map(said),
            flags_fault,
        });
    }
    Ok(checked)
}

/// `check`, and the default harness's login, which `check` leaves to the
/// look that follows it. A startup probe records this. It refuses nothing:
/// one harness that cannot start does not stop the probe.
pub fn observe(launches: &Launches, called: &[(Harness, Vec<String>)]) -> Vec<Checked> {
    let Ok(mut checked) = check(launches, called) else { return vec![] };
    for row in &mut checked {
        if row.tool.is_none() {
            row.tool = launches.of(row.harness).ok().flatten().as_ref().and_then(tools::harness);
        }
    }
    checked
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
    let _team = current(repo, bound, labels);
    let _ = launches;
    // Eligibility is checked per candidate at selection. One unavailable
    // harness cannot prevent another role or checkout from making progress.
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
            checked.tool = launches.of(harness)?.as_ref().and_then(tools::harness);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let worktree = root.join("worktree");
        let role = root.join("role");
        let team = root.join("team").join("CLAUDE.md");
        std::fs::create_dir(&worktree).unwrap();
        std::fs::create_dir(&role).unwrap();
        std::fs::create_dir(team.parent().unwrap()).unwrap();
        std::fs::write(worktree.join("AGENTS.md"), "the repo's own rules\n").unwrap();
        std::fs::write(worktree.join("CLAUDE.md"), "the repo's claude rules\n").unwrap();
        std::fs::write(&team, "the team rules\n").unwrap();
        // Present for every harness. Only one whose harness names this file is told it.
        std::fs::write(role.join("CLAUDE.md"), "the role's rules\n").unwrap();
        for harness in Harness::ALL {
            if let Some(name) = harness.role_file() {
                std::fs::write(role.join(name), format!("the role file {name}\n")).unwrap();
            }
        }
        (worktree, role, team)
    }

    #[test]
    fn the_role_file_a_worker_is_told_is_the_one_its_harness_names() {
        let root = tempfile::tempdir().unwrap();
        let (worktree, role, team) = rules(root.path());
        for harness in Harness::ALL {
            let read = ToldRules::read(harness, &worktree, team.to_str(), &role).unwrap();
            let told = read.told();
            match harness.role_file() {
                Some(name) => {
                    let rules = told.role.unwrap_or_else(|| panic!("{}", harness.key()));
                    assert!(rules.file.ends_with(name), "{}: {}", harness.key(), rules.file);
                    assert_eq!(rules.text, format!("the role file {name}\n"), "{}", harness.key());
                }
                None => assert!(told.role.is_none(), "{}", harness.key()),
            }
            if harness.told().is_empty() {
                assert!(told.repo.is_none() && told.team.is_none(), "{}", harness.key());
            } else {
                let repo = told.repo.unwrap_or_else(|| panic!("{}", harness.key()));
                assert!(repo.file.ends_with("AGENTS.md"), "{}: {}", harness.key(), repo.file);
                assert_eq!(repo.text, "the repo's own rules\n");
                let team_rules = told.team.unwrap_or_else(|| panic!("{}", harness.key()));
                assert_eq!(team_rules.text, "the team rules\n");
            }
        }
    }

    #[test]
    fn a_named_role_file_that_is_absent_is_not_told() {
        let root = tempfile::tempdir().unwrap();
        let (worktree, role, team) = rules(root.path());
        let mut removed = Vec::new();
        for harness in Harness::ALL {
            if let Some(name) = harness.role_file()
                && !removed.contains(&name)
            {
                std::fs::remove_file(role.join(name)).unwrap();
                removed.push(name);
            }
        }
        for harness in Harness::ALL {
            let read = ToldRules::read(harness, &worktree, team.to_str(), &role).unwrap();
            let told = read.told();
            assert!(told.role.is_none(), "{}", harness.key());
            if !harness.told().is_empty() {
                assert!(told.repo.is_some() && told.team.is_some(), "{}", harness.key());
            }
        }
    }
}

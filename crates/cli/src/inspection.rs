//! Inspections, as `run` starts and ends them (docs/adr/0019): once what
//! shows a worker hot is taken down (`evidence`), a worker in the role of its
//! stage's consult — the stock consult where the stage names none, and in an
//! unbound repo — is started in a checkout of its own and asked what is taking
//! so much and why. It says what it found with `afkconvoy inspected`; the
//! fleet types it at the hot worker, leaves it on the issue, and ends it.
//! Who inspects, which hot worker is next and what is typed are the core's
//! (`afkconvoy_core::inspection`).
//!
//! One inspection runs on the host at a time, across its fleets: the `run`
//! whose inspection runs holds the host's turn — a file of the cache it keeps
//! locked, saying what runs — and another hot worker waits for it. A `run`
//! that dies lets the turn go with it. An inspection takes no slot of the
//! `concurrency`.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use afkconvoy_core::config::Bound;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::hot::Limits;
use afkconvoy_core::inspection::{
    self, CHECKOUT, Fields, Finding, Found, Hot, Inspected, Inspector as Who, inspector, over,
};
use afkconvoy_core::role::{Facts, On, Start, workflow_of};
use afkconvoy_core::stage_record::history;
use afkconvoy_core::stock;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::copies::{Copies, Copy, cache};
use crate::dispatch::Agent;
use crate::evidence;
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::hot::{HotNow, held_said};
use crate::log::What;
use crate::orca::{Source, Worktree};
use crate::role::ToldRules;
use crate::stage::records_of;
use crate::team::keep;

/// The host's turn to inspect, in the cache every fleet of the host shares.
const TURN: &str = "inspection.lock";

/// An inspection running on the host, as its turn says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    /// The main checkout of the repo whose fleet runs it.
    pub repo: String,
    /// The hot worker it inspects, and the issue that worker is on.
    pub worker: String,
    pub issue: Option<u64>,
    /// The role it was started in.
    pub by: String,
    /// When it started, in epoch seconds.
    pub since: u64,
}

fn turn_file() -> Result<PathBuf> {
    Ok(cache()?.join(TURN))
}

/// Takes the host's turn to inspect for `running` → the file held locked for
/// as long as it lives; `None` while another inspection holds it.
fn take_turn(running: &Running) -> Result<Option<File>> {
    let path = turn_file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => {
            return Err(error).with_context(|| format!("cannot lock {}", path.display()));
        }
    }
    let said = serde_json::to_vec(running)?;
    file.set_len(0).and_then(|()| file.write_all_at(&said, 0))?;
    Ok(Some(file))
}

/// The inspection running on this host now, across its fleets; none where
/// none holds the turn.
pub fn on_host() -> Option<Running> {
    let mut file = File::open(turn_file().ok()?).ok()?;
    match file.try_lock() {
        // Nobody holds it: nothing runs. The lock goes with the file.
        Ok(()) => None,
        Err(TryLockError::WouldBlock) => {
            let mut said = String::new();
            file.read_to_string(&mut said).ok()?;
            serde_json::from_str(&said).ok()
        }
        Err(TryLockError::Error(_)) => None,
    }
}

/// `running`, as `doctor` and the panel show it.
pub fn json(running: &Running) -> Value {
    json!({"repo": running.repo, "worker": running.worker, "issue": running.issue,
        "by": running.by, "since": running.since})
}

/// An inspection this `run` started, while it runs.
struct Inspection {
    hot: HotNow,
    why: String,
    /// The hot worker's terminal, as the inspection started: what is typed at.
    terminal: Option<String>,
    checkout: Worktree,
    started: u64,
    by: String,
    /// The host's turn, held until it ends.
    _turn: File,
}

/// What a start came to.
enum Started {
    Running(Box<Inspection>),
    /// Nobody inspects the worker, for this reason.
    Nobody(String),
    /// Another inspection holds the host's turn.
    Waits,
}

/// Who an inspection is started as, and how.
enum As {
    /// The stock consult of an unbound repo, started with the repo's one
    /// command: its instruction is in its brief, and nothing guards it.
    Unbound,
    /// A role's consult, or the stock consult, of a bound repo: started in
    /// its role, guarded as a consult.
    Bound {
        harness: Harness,
        role: String,
        skill: String,
        role_copy: Copy,
        team_copy: Copy,
        team_rules: Option<String>,
        host_user: bool,
        escalate_label: String,
    },
}

impl As {
    fn role(&self) -> &str {
        match self {
            As::Unbound => stock::ROLE,
            As::Bound { role, .. } => role,
        }
    }
}

/// Who inspects the hot worker on `issue` — none of a patrol's — in a repo
/// `bound` to a team: its stage's consult, as the issue's records have it now
/// (`inspection::inspector`). `None`: nobody does.
fn bound_as(fleet: &Fleet, bound: &Bound, issue: Option<u64>) -> Result<Option<As>> {
    let copies = Copies::here()?;
    let kept = keep(&fleet.gh, &copies, &bound.team, &fleet.config.labels())?;
    let read = match issue {
        Some(issue) => Some((fleet.gh.issue(issue)?.labels, records_of(&fleet.gh, issue)?.0)),
        None => None,
    };
    let workflow = read
        .as_ref()
        .and_then(|(labels, records)| Some((workflow_of(&kept.team, labels).ok()?, records)));
    let place = workflow.map(|(workflow, records)| history(workflow, records).place);
    let stage = place.as_ref().and_then(|place| place.position.stage());
    let (harness, role, skill, role_copy, host_user) = match inspector(stage) {
        None => return Ok(None),
        // A consult a stage names runs on the repo's default harness.
        Some(Who::Role(work)) => {
            let role = kept.team.role_of(work);
            let copy = kept.role(role, bound.harness)?;
            (bound.harness, role.name.clone(), work.skill.clone(), copy, role.host_user)
        }
        // The stock consult runs on the stage's.
        Some(Who::Stock) => (
            stage.and_then(|stage| stage.harness(bound.harness)).unwrap_or(bound.harness),
            stock::ROLE.to_string(),
            stock::SKILL.to_string(),
            copies.stock()?,
            kept.team.host_user,
        ),
    };
    Ok(Some(As::Bound {
        harness,
        role,
        skill,
        role_copy,
        team_copy: kept.copy.clone(),
        team_rules: kept.rules(),
        host_user,
        escalate_label: kept.team.escalate_label.clone(),
    }))
}

/// What a `run` keeps of the inspections it starts.
pub struct Inspector {
    limits: Limits,
    inspected: Inspected,
    running: Option<Box<Inspection>>,
}

impl Inspector {
    /// The inspector of a `run` whose hot workers are found under `limits`.
    pub fn new(limits: Limits) -> Inspector {
        Inspector { limits, inspected: Inspected::default(), running: None }
    }

    /// The hot workers `hot`, as the core's rules see them.
    fn seen(hot: &[HotNow]) -> Vec<Hot> {
        let seen = hot.iter().map(|hot| Hot {
            worker: hot.path.display().to_string(),
            since: hot.episode.since,
            evidence: evidence::of(&hot.worktree(), &hot.worker, hot.episode.since).is_some(),
        });
        seen.collect()
    }

    /// Whether it is to look again soon: its inspection runs, or a hot worker
    /// of `hot` waits for one.
    pub fn busy(&self, hot: &[HotNow]) -> bool {
        self.running.is_some() || self.inspected.next(&Inspector::seen(hot)).is_some()
    }

    /// One look, at `now`, with `hot` the workers hot now: ends the
    /// inspection that is over, and starts the next a hot worker waits for,
    /// once the host's turn is free → what to log of it.
    pub fn step(&mut self, agent: &Agent, hot: &[HotNow], now: u64) -> Vec<What> {
        let mut said = vec![];
        if let Some(running) = &self.running {
            let gone = matches!(running.checkout.state(now), Ok(WorkerState::Gone));
            let finding = running.checkout.inspected().ok().flatten();
            if !over(running.started, finding.is_some(), gone, now) {
                return said;
            }
            let running = self.running.take().expect("running above");
            said.push(end(running, finding));
        }
        let seen = Inspector::seen(hot);
        self.inspected.keep_to(&seen);
        let Some(next) = self.inspected.next(&seen) else { return said };
        let found = hot.iter().find(|hot| {
            hot.path.display().to_string() == next.worker && hot.episode.since == next.since
        });
        let Some(hot) = found else { return said };
        let worker = hot.worker.clone();
        match self.start(agent, hot, now) {
            Ok(Started::Waits) => {}
            Ok(Started::Nobody(why)) => {
                self.inspected.record(next);
                said.push(What::Inspected { worker, found: "none".into(), said: why });
            }
            Ok(Started::Running(inspection)) => {
                self.inspected.record(next);
                said.push(What::Inspecting { worker, by: inspection.by.clone() });
                self.running = Some(inspection);
            }
            Err(error) => {
                self.inspected.record(next);
                let why = format!("it could not be started: {error:#}");
                said.push(What::Inspected { worker, found: "failed".into(), said: why });
            }
        }
        said
    }

    /// Starts the inspection of `hot` at `now`, in a checkout of the base
    /// branch of its own, once the host's turn is taken.
    fn start(&self, agent: &Agent, hot: &HotNow, now: u64) -> Result<Started> {
        // Asked first: a worker waiting its turn reads nothing of GitHub.
        if on_host().is_some() {
            return Ok(Started::Waits);
        }
        let fleet = Fleet::open(now)?;
        let worktree = hot.worktree();
        let evidence = evidence::of(&worktree, &hot.worker, hot.episode.since)
            .ok_or_else(|| anyhow!("what shows {} hot is not taken down", hot.worker))?;
        let who = match &fleet.config.bound {
            Some(bound) => bound_as(&fleet, bound, hot.issue)?,
            None => fleet.config.stock_consult().then_some(As::Unbound),
        };
        let Some(who) = who else {
            let why = match fleet.config.bound {
                Some(_) => "nobody inspects it: its stage says `human`",
                None => "nobody inspects it: the repo says `escalate_to: human`",
            };
            return Ok(Started::Nobody(why.to_string()));
        };
        let harness = match &who {
            As::Unbound => fleet.config.harness(),
            As::Bound { harness, .. } => *harness,
        };
        let launch = agent.command(harness)?;
        let running = Running {
            repo: fleet.repo.main_checkout()?.display().to_string(),
            worker: hot.worker.clone(),
            issue: hot.issue,
            by: who.role().to_string(),
            since: now,
        };
        let Some(turn) = take_turn(&running)? else { return Ok(Started::Waits) };
        let source = Source::of(&fleet.repo)?;
        if let Some(left) = Worktree::named(&source, CHECKOUT)? {
            left.remove()?;
        }
        let checkout = Worktree::cut(&source, CHECKOUT, &fleet.base_tip()?, None)?;
        let why = hot.why(&self.limits);
        let terminal = worktree.terminal()?;
        let started = (|| {
            let path = checkout.path.display().to_string();
            let fields = Fields {
                worker: &hot.worker,
                issue: hot.issue,
                why: &why,
                held: &held_said(&hot.episode),
                worktree: &hot.path.display().to_string(),
                terminal: terminal.as_deref().unwrap_or("(it has none)"),
                evidence: &evidence.display().to_string(),
                checkout: &path,
                role: who.role(),
                skill: match &who {
                    As::Unbound => stock::SKILL,
                    As::Bound { skill, .. } => skill,
                },
                instruction: matches!(who, As::Unbound).then_some(stock::INSTRUCTION),
                inspected_command: &crate::own_command_in(&path).fixed(" inspected").own(),
            };
            let brief =
                checkout.write_brief(&inspection::brief(&fields).map_err(|e| anyhow!(e))?)?;
            let command = command(&who, launch.as_str(), &checkout, &fleet, &hot.worker)?;
            let handle = checkout.start(&command, now)?;
            checkout.await_ready(&handle, agent.ready_timeout)?;
            checkout.deliver(&handle, &brief)
        })();
        if let Err(error) = started {
            // Nothing is left of an inspection that did not start.
            let _ = checkout.remove();
            return Err(error);
        }
        Ok(Started::Running(Box::new(Inspection {
            hot: hot.clone(),
            why,
            terminal,
            checkout,
            started: now,
            by: who.role().to_string(),
            _turn: turn,
        })))
    }
}

/// The line the inspection's terminal in `checkout` runs, `launch` being the
/// worker launch command of its harness: in a bound repo, with what starts it
/// in its role and holds it as a consult.
fn command(
    who: &As,
    launch: &str,
    checkout: &Worktree,
    fleet: &Fleet,
    worker: &str,
) -> Result<String> {
    let As::Bound {
        harness,
        role,
        skill,
        role_copy,
        team_copy,
        team_rules,
        host_user,
        escalate_label,
    } = who
    else {
        return Ok(launch.to_string());
    };
    let told = ToldRules::read(*harness, &checkout.path, team_rules.as_deref())?;
    let (kept, shadowed) = crate::kept::for_worker(*harness, *host_user, &role_copy.dir)?;
    let start = Start {
        role_dir: &role_copy.dir.display().to_string(),
        worktree: &checkout.path.display().to_string(),
        team_dir: &team_copy.dir.display().to_string(),
        model: None,
        guard: &crate::own_command().fixed(" guard").own(),
        kept: &kept,
        host_user: *host_user,
        shadowed: &shadowed,
        told: told.told(),
        facts: Facts {
            on: On::Inspection(worker),
            role,
            skill,
            writes: false,
            consult: true,
            base_branch: &fleet.config.base_branch,
            escalate_label,
            lock_label: &fleet.config.lock_label,
            role_commit: &role_copy.commit,
            team_commit: &team_copy.commit,
        },
    };
    Ok(harness.in_role(launch, &start))
}

/// Ends `inspection`, which found `finding`: types it at the hot worker —
/// not what is not its own — leaves it on the issue, and removes the
/// inspection's checkout, its terminal with it → what to log of it. What
/// cannot be done of it is said, and the rest done all the same.
fn end(inspection: Box<Inspection>, finding: Option<Finding>) -> What {
    let worker = inspection.hot.worker.clone();
    let mut said = match &finding {
        Some(finding) => finding.said.split_whitespace().collect::<Vec<_>>().join(" "),
        None => format!("it said nothing in {} minutes", inspection::TIME_LIMIT / 60),
    };
    let typed = finding.as_ref().and_then(Finding::typed);
    match (typed, &inspection.terminal) {
        (Some(line), Some(terminal)) => {
            if let Err(error) = inspection.hot.worktree().tell(terminal, &line) {
                said.push_str(&format!(" · it could not be typed at the worker: {error:#}"));
            }
        }
        (Some(_), None) => said.push_str(" · the worker had no terminal to type it at"),
        (None, _) => {}
    }
    if let Some(issue) = inspection.hot.issue {
        let body = inspection::comment(&worker, &inspection.why, &inspection.by, finding.as_ref());
        let commented =
            Repo::here().and_then(|repo| GitHub { dir: repo.dir }.comment(issue, &body));
        if let Err(error) = commented {
            said.push_str(&format!(" · no comment was left on #{issue}: {error:#}"));
        }
    }
    if let Err(error) = inspection.checkout.remove() {
        said.push_str(&format!(" · its checkout was not removed: {error:#}"));
    }
    let found = finding.map_or("nothing", |finding| finding.found.word());
    What::Inspected { worker, found: found.to_string(), said }
}

/// `afkconvoy inspected`: keeps what the inspection in this checkout found,
/// for its `run` to type at the hot worker and leave on its issue.
pub fn inspected(found: Found, said: &str) -> Result<Value> {
    if said.trim().is_empty() {
        anyhow::bail!("say what was found, where and why: the text is empty");
    }
    let checkout = Worktree::here()?;
    let named = checkout.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if !afkconvoy_core::worker::is_worktree_named(named, CHECKOUT) {
        anyhow::bail!(
            "{} is no inspection's checkout: only an inspection says what it found, from \
             the checkout it was started in",
            checkout.path.display()
        );
    }
    checkout.record_inspected(&Finding { found, said: said.trim().to_string() })?;
    Ok(json!({"inspected": true, "found": found.word()}))
}

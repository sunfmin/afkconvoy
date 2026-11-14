//! The working set: everything a tick acts on, assembled from what GitHub and
//! the remote's refs say now. The same state gives the same working set,
//! whatever order the reads returned it in.

use serde::Serialize;

use crate::admission::Hold;
use crate::claims::{Claim, InstanceId};
use crate::config::Config;
use crate::cut;
use crate::lease::is_stale;
use crate::lock;
use crate::patrol::{self, Standing};
use crate::role::Due;
use crate::slow_gate::{Landing, Reported, SlowGate, slow};
use crate::team::{FlowLabels, LAND};
use crate::tracker::{Tracker, repo_labels};
use crate::waiting::{Waiting, Waits};

/// One open issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    /// The open issues that block it, lowest number first.
    pub open_blockers: Vec<u64>,
}

/// One open pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    pub head_sha: String,
    pub base_branch: String,
    /// The issues of the fleet's tracker it closes (`Tracker::closed_by`).
    pub closes: Vec<u64>,
}

impl PullRequest {
    /// This PR as it is once its branch is at `head`.
    pub fn at_head(&self, head: &str) -> PullRequest {
        PullRequest { head_sha: head.to_string(), ..self.clone() }
    }
}

/// The open PR that closes issue `number`: the highest-numbered when several
/// do — the latest attempt is the live one.
pub fn closing_pr(prs: &[PullRequest], number: u64) -> Option<&PullRequest> {
    let mut latest: Option<&PullRequest> = None;
    for pr in prs {
        if pr.closes.contains(&number) && latest.is_none_or(|found| pr.number > found.number) {
            latest = Some(pr);
        }
    }
    latest
}

/// Why an unclaimed issue of an unbound repo is held off when an open PR it
/// is closed by is not the fleet's: a worker would open a second one.
pub const NOT_OURS: &str = "has an open linked PR";

/// Fleet repairs run as operator workers, outside product workflows. The
/// ready label still controls whether a repair may be dispatched.
pub const REPAIR_LABEL: &str = "convoy-repair";

pub fn is_repair(labels: &[String]) -> bool {
    labels.iter().any(|label| label == REPAIR_LABEL)
}

/// Why an issue's labels keep a fleet from dispatching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelBar {
    /// It carries this epic label: it is never dispatched, whoever claims it.
    Epic(String),
    /// It lacks this, the ready label.
    NotReady(String),
    /// A bound repo's issue that carries no flow label: it was not handed to
    /// the fleet.
    NoFlow,
    /// It carries these repo labels, of more than one repo: no repo's fleet
    /// works it (docs/adr/0027).
    Repos(Vec<String>),
}

impl LabelBar {
    pub fn reason(&self) -> String {
        match self {
            LabelBar::Epic(label) => format!("epic label ({label})"),
            LabelBar::NotReady(ready) => format!("no {ready} label"),
            LabelBar::NoFlow => "no flow label".to_string(),
            LabelBar::Repos(labels) => {
                format!("repo labels of more than one repo ({})", labels.join(", "))
            }
        }
    }
}

/// What in an issue's labels keeps a fleet from dispatching it; `None` when
/// they allow it. `flows` are the flow labels of a bound repo's team. The one
/// home of the label rules: the frontier reads it, and so does the standing
/// of a blocker, so the two cannot disagree. An epic is barred whatever else
/// it carries: the ready label too, where that is a flow label.
pub fn label_bar(labels: &[String], config: &Config, flows: &FlowLabels) -> Option<LabelBar> {
    if let Some(epic) = labels.iter().find(|label| config.epic_labels.contains(label)) {
        return Some(LabelBar::Epic(epic.clone()));
    }
    let repos = repo_labels(labels);
    if repos.len() > 1 {
        return Some(LabelBar::Repos(repos.into_iter().map(String::from).collect()));
    }
    // The ready label means something only in an unbound repo.
    match &config.bound {
        Some(_) if is_repair(labels) => {
            if labels.contains(&config.ready_label) {
                None
            } else {
                Some(LabelBar::NotReady(config.ready_label.clone()))
            }
        }
        Some(_) if flows.among(labels).is_empty() => Some(LabelBar::NoFlow),
        None if !labels.contains(&config.ready_label) => {
            Some(LabelBar::NotReady(config.ready_label.clone()))
        }
        _ => None,
    }
}

/// The label among `labels` that hands an issue to the fleet: in a bound repo
/// its flow label — one of `flows`, its team's — in an unbound one the ready
/// label; `None` when it carries none.
pub fn handed_by(labels: &[String], config: &Config, flows: &FlowLabels) -> Option<String> {
    match &config.bound {
        Some(_) if !is_repair(labels) => flows.among(labels).first().map(|flow| flow.to_string()),
        _ => labels.contains(&config.ready_label).then(|| config.ready_label.clone()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dispatchable {
    pub number: u64,
    pub title: String,
    /// The stage a worker is to be started on, or `land` for the join that
    /// follows the last one. `None` in an unbound repo, whose issues have no
    /// stages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// In an unbound repo, the harness and model its worker would run, and
    /// why (`casting::preferred`); `None` of a bound repo's issue, the stock
    /// consult's, and any while the host kit cannot be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cast: Option<crate::casting::Cast>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Excluded {
    pub number: u64,
    pub title: String,
    pub reason: String,
}

/// An open blocker no checkout serving the tracker works, with the repair
/// lines read from the host. It holds build work back, but never a hand-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnworkedBlocker {
    pub issue: u64,
    pub blocker: u64,
    pub repo: String,
    pub reason: String,
    pub fix: String,
}

/// Where one of my claims stands: what the tick does with it next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// No open PR closes the issue: its worker is still at it, or stopped.
    NoPr,
    /// An open PR closes it, and that PR is not on the base branch yet. In
    /// a bound repo, only under a landing worker's claim.
    Landing,
    /// The landing train put its work on the base branch, and the landing was
    /// cut short before the issue was closed: to be closed, then settled.
    Landed,
    /// The issue is closed: the claim is to be settled and released.
    Closed,
    /// GitHub says the issue was deleted, transferred to another repo, or
    /// never issued: nothing is left to settle, and the claim is let go.
    Gone,
    /// A bound repo's claim on an open issue that is not a landing worker's:
    /// it was dispatched to a stage, and what that stage declares is what
    /// ends it. An open PR says nothing.
    AtStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mine {
    pub number: u64,
    /// `None` when the issue is closed or gone: only open issues are gathered.
    pub title: Option<String>,
    pub status: Status,
    pub pr: Option<u64>,
    /// The stage a worker is due on, were the issue unclaimed: where the
    /// frontier would hold it — at a stage a role performs, never for a
    /// consult or the join. A claim that ends with its issue left there
    /// (`tick::Outcome::HandedOn`) leaves that worker for the pass to start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// A verdict was declared under this claim — not before it was created —
    /// and is not acted on yet: the claim is held until it is, and takes no
    /// slot meanwhile. Only of a claim with no PR, or at a stage.
    pub declared: bool,
    /// A landing claim whose worker is not changing the tree: gone or idle,
    /// or its turn queued, waiting on a predecessor, or answered with a wait
    /// (`landing_worker::changing_tree`). The claim is held until the PR
    /// lands or it ends otherwise, and takes no slot meanwhile.
    pub idle: bool,
}

impl Mine {
    /// Whether the claim takes no slot: its worker declared, or it is landing
    /// and its worker is not changing the tree.
    pub fn slotless(&self) -> bool {
        self.declared || self.idle
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerClaim {
    pub number: u64,
    /// `None`: the claim's record names nobody.
    pub instance: Option<String>,
    /// What a take or a delete of the claim is leased to.
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkingSet {
    #[serde(skip)]
    pub unworked_blockers: Vec<UnworkedBlocker>,
    /// The issues a worker can be started on right now: in a bound repo,
    /// each at the stage its stage records put it at, those a stage has
    /// already declared on ahead of those never started — work somebody
    /// began. Lowest number first within each.
    pub frontier: Vec<Dispatchable>,
    /// A bound repo's unclaimed issues to hand to a human, each with the
    /// reason: a flow label that names no one workflow, or a position that
    /// is an escalation nobody carried out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub escalating: Vec<Excluded>,
    /// Every other open issue, with the reason it is not on the frontier.
    pub excluded: Vec<Excluded>,
    /// Every open issue nobody holds that waits for a human, each with the
    /// line that answers it, lowest number first: the escalated ones, those
    /// at a stage that is a human's or past the end of their workflow, and
    /// the reports of a patrol's failed run.
    pub waiting_on_human: Vec<Waiting>,
    pub mine: Vec<Mine>,
    /// Claims of peers whose heartbeat is within the lease: never touched.
    pub peer_live: Vec<PeerClaim>,
    /// Claims of peers presumed dead, on open issues: work to take.
    pub stale: Vec<PeerClaim>,
    /// Claims of peers presumed dead, on closed or gone issues: locks to
    /// delete.
    pub stale_closed: Vec<PeerClaim>,
    /// The claimed issues GitHub says were deleted, transferred or never
    /// issued, whoever
    /// holds the claim, lowest number first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<u64>,
    pub now: u64,
    /// The fleet locks standing: the open issues carrying the lock label,
    /// lowest number first. While there is one, the frontier holds lock issues
    /// only, and every other issue the fleet would have worked is excluded.
    pub locks: Vec<u64>,
    /// A bound repo's open issues that carry a flow label: those with a
    /// position in a workflow, whoever holds them.
    #[serde(skip)]
    pub flowing: Vec<Issue>,
    /// The patrols a bound repo names, each as it stands now, in the order
    /// the repo names them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub patrols: Vec<patrol::Row>,
    /// The gate is slow, with the evidence and the open issue that says so:
    /// a pass opens one while there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slow_gate: Option<SlowGate>,
    /// The thresholds of its host that admission is past (`admission::holds`):
    /// while there is one, a pass starts no new worker into a free slot. Read
    /// of the host, not of GitHub: what assembles the set leaves it empty,
    /// and the effect layer puts in what it read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub held_back: Vec<Hold>,
    /// Whether each open issue's model label is followed, or its worker runs
    /// the user's default, and the recorded model of a worker already running.
    pub casting: Vec<crate::casting::ModelStatus>,
    /// The repo's cut (docs/adr/0028), as it stands now: whether a pass that
    /// ends idle starts one, or asks after one of mine. Left out of a set
    /// assembled with nothing read of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut: Option<cut::Row>,
}

impl WorkingSet {
    /// Whether a cut of mine is going, as this set was read: it takes no
    /// slot, and keeps the fleet instance from being idle.
    pub fn cut_running(&self) -> bool {
        self.cut.as_ref().is_some_and(|cut| cut.standing == cut::Standing::Running)
    }

    /// Whether a fleet lock keeps the fleet off `issue` (`lock::keeps_off`).
    pub fn locked_out(&self, issue: u64) -> bool {
        lock::keeps_off(&self.locks, issue)
    }

    /// The slots taken as a pass starts: one for each claim of mine, and one
    /// for each patrol whose run of mine is still going.
    pub fn held(&self) -> usize {
        let mut held = self.mine.len();
        for patrol in &self.patrols {
            if patrol.standing == Standing::Running {
                held = held.saturating_add(1);
            }
        }
        held
    }

    /// How many claims of mine a standing fleet lock has a pass leave alone:
    /// those on another issue whose worker is not on the train yet. None
    /// while no lock stands.
    pub fn left_alone(&self) -> usize {
        let mut left_alone: usize = 0;
        for mine in &self.mine {
            let at_work = matches!(mine.status, Status::NoPr | Status::Landing | Status::AtStage);
            if at_work && self.locked_out(mine.number) {
                left_alone = left_alone.saturating_add(1);
            }
        }
        left_alone
    }

    /// How many claims of mine take no slot (`Mine::slotless`): a verdict was
    /// declared under each, or each is landing with its worker not changing
    /// the tree. Not those a fleet lock has the pass leave alone: they are
    /// counted there.
    pub fn slotless(&self) -> usize {
        let mut declared: usize = 0;
        for mine in &self.mine {
            if mine.slotless() && !self.locked_out(mine.number) {
                declared = declared.saturating_add(1);
            }
        }
        declared
    }

    /// The claims a pass may hold under `base`: one more for each claim of mine whose worker declared a
    /// verdict, which is held only until the grace of its worker runs out —
    /// the frontier does not wait that out — one more for each landing claim
    /// of mine whose worker is not changing the tree, which is held until its
    /// PR lands, and, with a fleet lock standing,
    /// one more for each claim of mine the pass leaves alone. Nothing reads
    /// what their workers declare until the lock ends, so a lock issue that
    /// waited for one of their slots would wait for the lock it is there to
    /// end.
    pub fn bound(&self, base: usize) -> usize {
        base.saturating_add(self.left_alone()).saturating_add(self.slotless())
    }

    /// Where open issue `number` stands, in a set assembled of nobody — every
    /// claim a peer's. `None`: the set holds no such open issue.
    pub fn stands(&self, number: u64) -> Option<Stands<'_>> {
        // Every open issue is on the frontier or kept off it.
        if self.frontier.iter().any(|issue| issue.number == number) {
            return Some(Stands::Ready);
        }
        let mut excluded = self.excluded.iter().chain(&self.escalating);
        let reason = &excluded.find(|row| row.number == number)?.reason;
        if let Some(waiting) = self.waiting_on_human.iter().find(|row| row.issue == number) {
            return Some(Stands::Waiting(waiting));
        }
        let live = self.peer_live.iter().find(|row| row.number == number);
        if let Some(claim) = live {
            return Some(Stands::Claimed { claim, live: true });
        }
        if let Some(claim) = self.stale.iter().find(|row| row.number == number) {
            return Some(Stands::Claimed { claim, live: false });
        }
        Some(Stands::Held(reason))
    }
}

/// Where one open issue stands among the facts a pass acts on.
#[derive(Debug)]
pub enum Stands<'s> {
    /// It waits for a human: escalated, or the report of a failed run.
    Waiting(&'s Waiting),
    /// A fleet instance holds it: `live` while its heartbeat is within the
    /// lease, and otherwise its claim is work to take.
    Claimed { claim: &'s PeerClaim, live: bool },
    /// A worker can be started on it now.
    Ready,
    /// It is kept off the frontier, for this reason.
    Held(&'s str),
}

/// With a fleet lock standing, takes every issue that is not one off the
/// frontier and out of what is handed to a human: it is excluded, the lock
/// being why.
fn hold_back(set: &mut WorkingSet) {
    if set.locks.is_empty() {
        return;
    }
    let why = lock::reason(&set.locks);
    let mut kept_off: Vec<(u64, String)> = vec![];
    for issue in &set.frontier {
        if lock::keeps_off(&set.locks, issue.number) {
            kept_off.push((issue.number, issue.title.clone()));
        }
    }
    for issue in &set.escalating {
        if lock::keeps_off(&set.locks, issue.number) {
            kept_off.push((issue.number, issue.title.clone()));
        }
    }
    set.frontier.retain(|issue| !lock::keeps_off(&set.locks, issue.number));
    set.escalating.retain(|issue| !lock::keeps_off(&set.locks, issue.number));
    for (number, title) in kept_off {
        set.excluded.push(Excluded { number, title, reason: why.clone() });
    }
    set.excluded.sort_by_key(|excluded| excluded.number);
}

/// What the effect layer read, in whatever order it came.
pub struct Observed<'a> {
    pub unworked_blockers: &'a [UnworkedBlocker],
    /// Issues whose open blockers are all unworked, compared by repo and number.
    pub handoffs: &'a [u64],
    pub issues: &'a [Issue],
    pub prs: &'a [PullRequest],
    pub claims: &'a [Claim],
    /// instance → the time of its heartbeat.
    pub heartbeats: &'a [(String, u64)],
    /// The claimed issues that are closed (`issues` holds only open ones).
    pub closed: &'a [u64],
    /// The claimed issues GitHub says were deleted, transferred or never
    /// issued.
    pub gone: &'a [u64],
    /// The open issues whose open PR is the fleet's own: its branch is one
    /// the fleet recorded on the issue. Unclaimed, it is a PR an escalation
    /// or a park kept, which a worker takes up.
    pub kept: &'a [u64],
    /// My open issues the landing train landed under my claim.
    pub landed: &'a [u64],
    /// My claimed issues with a verdict declared under the claim and not
    /// acted on yet (`Mine::declared`).
    pub declared: &'a [u64],
    /// My claimed issues whose PR is open and whose worker is not changing
    /// the tree (`Mine::idle`).
    pub idle: &'a [u64],
    /// What is due on each open issue of a bound repo that carries a flow
    /// label (`role::due`). Empty in an unbound repo.
    pub due: &'a [(u64, Due<'a>)],
    /// The flow labels of a bound repo's team, as the rebuild read it: none
    /// in an unbound repo, or where the team did not load.
    pub flows: &'a FlowLabels,
    /// The claimed issues of a bound repo whose claim is a landing worker's
    /// (`stage_record::lands`). Empty in an unbound repo.
    pub landing: &'a [u64],
    /// The open issues of a bound repo that have a stage record: a worker
    /// was started on each before. Empty in an unbound repo.
    pub begun: &'a [u64],
    /// The open issues whose named files overlap those the claimed issues
    /// name (`apart::overlapping`): a pass starts them after the rest, so
    /// that parallel work rarely conflicts (docs/adr/0076).
    pub overlapping: &'a [u64],
    /// The patrols the repo names (`patrol::Seen`), in its order. Empty in
    /// an unbound repo.
    pub patrols: &'a [patrol::Seen],
    /// The open issues that wait for a human, each with what for
    /// (`waiting::Waits`) and since when, where GitHub said. The reports of
    /// a patrol's failed run are not among them: `patrols` names those.
    pub waiting: &'a [(u64, Waits, Option<u64>)],
    /// The base branch's landings with a recorded run of the gate command,
    /// newest first.
    pub landings: &'a [Landing],
    /// The issues the fleet opened for a slow gate.
    pub slow_gate: &'a Reported,
    /// The fleet's tracker, which holds `issues`: the lines that answer one
    /// that waits for a human name it (`Waiting::new`).
    pub tracker: &'a Tracker,
    /// What was read of the repo's cut (`cut::Seen`); `None` where nothing
    /// was, and the set holds no cut.
    pub cut: Option<&'a cut::Seen>,
}

/// What a bound repo's open issue calls for by what is due on it, whoever
/// holds it. The one home of that rule: the frontier reads it, and so does
/// what a claim of mine leaves to start once it ends.
enum Call<'d> {
    /// A worker is to be started at this stage — `land` for the join.
    Start(String, &'d Due<'d>),
    /// The issue is to be handed to a human, for this reason.
    Human(String),
    /// Nothing, for this reason.
    Nothing(String),
}

fn blocker_reason(blockers: &[u64]) -> String {
    let named: Vec<String> = blockers.iter().map(|number| format!("#{number}")).collect();
    format!("open blockers: {}", named.join(", "))
}

fn call<'d>(issue: &Issue, due: &'d [(u64, Due<'d>)], handoffs: &[u64]) -> Call<'d> {
    // Where its stage records put it decides. An open PR declares nothing.
    let due = due.iter().find(|(number, _)| *number == issue.number).map(|(_, due)| due);
    let handing = matches!(due, Some(Due::Consult(_))) && handoffs.contains(&issue.number);
    let blocked = !issue.open_blockers.is_empty() && !handing;
    match due {
        Some(Due::Human(reason)) => Call::Human(reason.clone()),
        Some(Due::Nothing(why)) => Call::Nothing(why.clone()),
        Some(Due::Worker(_) | Due::Consult(_) | Due::Lander(_)) if blocked => {
            Call::Nothing(blocker_reason(&issue.open_blockers))
        }
        Some(due @ Due::Lander(_)) => Call::Start(LAND.to_string(), due),
        // A consult is started at the stage that asked it.
        Some(due @ (Due::Worker(entry) | Due::Consult(entry))) => {
            Call::Start(entry.stage.name.clone(), due)
        }
        None => Call::Nothing("its position was not read".to_string()),
    }
}

/// The open issues that wait for a human, lowest number first: those `seen`
/// names, the report of each failed patrol among `patrols`, and that of a
/// failed cut handed to a human. An issue a fleet instance holds waits for
/// nobody yet: its escalation is under way.
fn waiting(
    seen: &Observed,
    issues: &[&Issue],
    claims: &[&Claim],
    patrols: &[patrol::Row],
    cut: Option<&cut::Row>,
) -> Vec<Waiting> {
    let mut rows: Vec<Waiting> = vec![];
    let failed = patrols.iter().filter_map(|patrol| {
        Some((patrol.failure?, Waits::PatrolFailed { patrol: patrol.name.clone() }, None))
    });
    // A failed cut's issue under the watch's label is the watch's, not a
    // human's: only one handed to a human, under the escalate label, waits.
    let watched = |number: u64| {
        let issue = issues.iter().find(|issue| issue.number == number);
        issue.is_some_and(|issue| cut::watched(&issue.labels))
    };
    let cut_failed = cut
        .and_then(|cut| cut.failure)
        .filter(|issue| !watched(*issue))
        .map(|issue| (issue, Waits::CutFailed, None));
    for (number, waits, since) in seen.waiting.iter().cloned().chain(failed).chain(cut_failed) {
        let open = issues.iter().find(|issue| issue.number == number);
        let held = claims.iter().any(|claim| claim.number == number);
        let listed = rows.iter().any(|row| row.issue == number);
        if let Some(issue) = open.filter(|_| !held && !listed) {
            rows.push(Waiting::new(number, seen.tracker, &issue.title, waits, since));
        }
    }
    rows.sort_by_key(|row| row.issue);
    rows
}

/// Assembles the working set of the fleet instance `me` (`None`: of nobody —
/// every claim is a peer's).
pub fn assemble(seen: &Observed, me: Option<&InstanceId>, now: u64, config: &Config) -> WorkingSet {
    let mut issues: Vec<&Issue> = seen.issues.iter().collect();
    issues.sort_by_key(|issue| issue.number);
    let mut claims: Vec<&Claim> = seen.claims.iter().collect();
    claims.sort_by_key(|claim| claim.number);

    let mut set = WorkingSet {
        unworked_blockers: seen.unworked_blockers.to_vec(),
        frontier: vec![],
        escalating: vec![],
        excluded: vec![],
        waiting_on_human: vec![],
        mine: vec![],
        peer_live: vec![],
        stale: vec![],
        stale_closed: vec![],
        gone: claims
            .iter()
            .map(|claim| claim.number)
            .filter(|number| seen.gone.contains(number))
            .collect(),
        now,
        locks: lock::standing(seen.issues, config),
        flowing: vec![],
        patrols: vec![],
        slow_gate: slow(
            seen.landings,
            config.gate_seconds(),
            config.gate_budget_seconds,
            seen.slow_gate,
        ),
        held_back: vec![],
        casting: vec![],
        cut: None,
    };
    let locked = !set.locks.is_empty();
    for patrol in seen.patrols {
        set.patrols.push(patrol::row(patrol, me, seen.heartbeats, now, locked));
    }
    if let Some(cut) = seen.cut {
        let mut row = cut::row(cut, me, seen.heartbeats, now, locked);
        for issue in &issues {
            if cut::waits(&issue.labels) {
                row.waiting.push(issue.number);
            }
        }
        set.cut = Some(row);
    }
    set.waiting_on_human = waiting(seen, &issues, &claims, &set.patrols, set.cut.as_ref());
    for issue in &issues {
        if config.bound.is_some() && !seen.flows.among(&issue.labels).is_empty() {
            set.flowing.push((*issue).clone());
        }
        let claimed = claims.iter().any(|claim| claim.number == issue.number);
        let (number, title) = (issue.number, issue.title.clone());
        let failed_cut = set.cut.as_ref().is_some_and(|cut| cut.failure == Some(number));
        // Made a ticket of the fleet's after `cut::REPEATS` failures of the
        // same way, it carries the ready label and is worked as any other
        // (docs/adr/0072).
        let ticket = issue.labels.contains(&config.ready_label);
        let reason = if failed_cut && cut::watched(&issue.labels) && !ticket {
            // The watch looks into it; nothing of the fleet's works it.
            "says a cut failed: the host's watch answers it".to_string()
        } else if let Some(bar) = label_bar(&issue.labels, config, seen.flows) {
            // An escalated issue lost the label that bars it to the escalation.
            let waits = set.waiting_on_human.iter().find(|waiting| waiting.issue == number);
            let said = waits.and_then(|waiting| waiting.waits.excluded());
            said.filter(|_| !matches!(bar, LabelBar::Epic(_))).unwrap_or_else(|| bar.reason())
        } else if claimed {
            "already claimed".to_string()
        } else if cut::waits(&issue.labels) {
            // Off the frontier until a cut installs a version: no worker
            // waits for it, and the fleet can go idle and cut.
            cut::WAITS.to_string()
        } else if config.bound.is_some() && !is_repair(&issue.labels) {
            match call(issue, seen.due, seen.handoffs) {
                Call::Human(reason) => {
                    set.escalating.push(Excluded { number, title, reason });
                    continue;
                }
                Call::Nothing(why) => why,
                Call::Start(stage, _) => {
                    set.frontier.push(Dispatchable {
                        number,
                        title,
                        stage: Some(stage),
                        cast: None,
                    });
                    continue;
                }
            }
        } else if closing_pr(seen.prs, number).is_some() && !seen.kept.contains(&number) {
            NOT_OURS.to_string()
        } else if !issue.open_blockers.is_empty() {
            blocker_reason(&issue.open_blockers)
        } else {
            set.frontier.push(Dispatchable { number, title, stage: None, cast: None });
            continue;
        };
        set.excluded.push(Excluded { number, title, reason });
    }
    // Stable: what was begun first, then what is apart from the work in
    // flight; lowest number first within each.
    set.frontier.sort_by_key(|issue| {
        (!seen.begun.contains(&issue.number), seen.overlapping.contains(&issue.number))
    });
    for claim in claims {
        let gone = seen.gone.contains(&claim.number);
        let closed = gone || seen.closed.contains(&claim.number);
        if me.is_some_and(|me| claim.is_held_by(me)) {
            let pr = closing_pr(seen.prs, claim.number).map(|pr| pr.number);
            let status = if gone {
                Status::Gone
            } else if closed {
                Status::Closed
            } else if seen.landed.contains(&claim.number) {
                Status::Landed
            } else if config.bound.is_some()
                && !issues
                    .iter()
                    .any(|issue| issue.number == claim.number && is_repair(&issue.labels))
                && !(pr.is_some() && seen.landing.contains(&claim.number))
            {
                // A landing worker's claim with no PR left to join has
                // nothing to do either, and is ended as a stage's is.
                Status::AtStage
            } else if pr.is_none() {
                Status::NoPr
            } else {
                Status::Landing
            };
            let issue = issues.iter().find(|issue| issue.number == claim.number);
            let title = issue.map(|issue| issue.title.clone());
            let startable = |issue: &&&Issue| {
                config.bound.is_some() && label_bar(&issue.labels, config, seen.flows).is_none()
            };
            let due = issue.filter(startable).and_then(|issue| {
                match call(issue, seen.due, seen.handoffs) {
                    Call::Start(stage, Due::Worker(_)) => Some(stage),
                    _ => None,
                }
            });
            let at_work = matches!(status, Status::NoPr | Status::AtStage);
            let declared = at_work && seen.declared.contains(&claim.number);
            let idle = status == Status::Landing && seen.idle.contains(&claim.number);
            set.mine.push(Mine { number: claim.number, title, status, pr, due, declared, idle });
            continue;
        }
        let instance = claim.owner.as_ref().map(|owner| owner.instance.clone());
        let beat = instance.as_ref().and_then(|instance| {
            seen.heartbeats.iter().find(|(name, _)| name == instance).map(|(_, ts)| *ts)
        });
        let row = PeerClaim { number: claim.number, instance, sha: claim.sha.clone() };
        if !is_stale(beat, now) {
            set.peer_live.push(row);
        } else if closed {
            set.stale_closed.push(row);
        } else {
            set.stale.push(row);
        }
    }
    hold_back(&mut set);
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;
    use crate::config::Settings;
    use crate::harness::Harness;
    use crate::lease::LEASE;

    fn config() -> Config {
        Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            ..Settings::default()
        })
        .unwrap()
    }

    fn issue(number: u64, labels: &[&str]) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            open_blockers: vec![],
        }
    }

    fn pr(number: u64, closes: u64) -> PullRequest {
        PullRequest {
            number,
            title: format!("pr {number}"),
            head_branch: format!("b{number}"),
            head_sha: format!("sha{number}"),
            base_branch: "main".into(),
            closes: vec![closes],
        }
    }

    fn claim(number: u64, instance: &str) -> Claim {
        let owner = Owner { instance: instance.into(), host: "h".into(), ts: 0 };
        Claim::read(number, &owner.write(), format!("c{number}"))
    }

    fn me() -> InstanceId {
        InstanceId::parse("me").unwrap()
    }

    const READY: &[&str] = &["ready-for-agent"];

    #[test]
    fn the_issues_apart_from_the_work_in_flight_go_first() {
        let issues = [issue(1, READY), issue(2, READY), issue(3, READY), issue(4, READY)];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[],
            claims: &[claim(4, "peer")],
            heartbeats: &[("peer".into(), 1000)],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[1, 3],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &config());
        let frontier: Vec<u64> = set.frontier.iter().map(|d| d.number).collect();
        assert_eq!(frontier, [2, 1, 3], "what overlaps waits behind, still in number order");
    }

    #[test]
    fn the_frontier_is_every_issue_nothing_bars() {
        let mut blocked = issue(6, READY);
        blocked.open_blockers = vec![41, 42];
        let issues = [
            issue(9, READY),
            issue(1, READY),
            issue(2, &["bug"]),
            issue(3, &["ready-for-agent", "epic"]),
            issue(4, READY),
            issue(5, READY),
            blocked,
            issue(7, &["ready-for-agent", "repo:acme/dev", "Repo:acme/web"]),
        ];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[pr(40, 5)],
            claims: &[claim(4, "peer")],
            heartbeats: &[("peer".into(), 1000)],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &config());
        let frontier: Vec<u64> = set.frontier.iter().map(|d| d.number).collect();
        assert_eq!(frontier, [1, 9], "in issue-number order");
        let reasons: Vec<(u64, &str)> =
            set.excluded.iter().map(|e| (e.number, e.reason.as_str())).collect();
        assert_eq!(
            reasons,
            [
                (2, "no ready-for-agent label"),
                (3, "epic label (epic)"),
                (4, "already claimed"),
                (5, "has an open linked PR"),
                (6, "open blockers: #41, #42"),
                (7, "repo labels of more than one repo (repo:acme/dev, Repo:acme/web)"),
            ]
        );
    }

    #[test]
    fn an_issue_waiting_on_the_cut_leaves_the_fleet_idle_until_a_cut_installs_a_version() {
        let waiting = [issue(8, &["ready-for-agent", cut::WAITING_LABEL])];
        let tag = cut::Tag { version: cut::Version::parse("0.1.5").unwrap(), commit: "t".into() };
        let due = cut::Seen {
            on: true,
            newest: Some(tag),
            past: 3,
            oldest_landing: Some(900),
            busy_after_seconds: cut::BUSY_AFTER_SECONDS,
            ..cut::Seen::default()
        };
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &waiting,
            prs: &[],
            claims: &[],
            heartbeats: &[],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: Some(&due),
        };
        let set = assemble(&seen, Some(&me()), 1000, &config());
        assert!(set.frontier.is_empty(), "{:?}", set.frontier);
        let reasons: Vec<(u64, &str)> =
            set.excluded.iter().map(|e| (e.number, e.reason.as_str())).collect();
        assert_eq!(reasons, [(8, cut::WAITS)]);
        let row = set.cut.as_ref().unwrap();
        assert_eq!((row.standing, &row.waiting[..]), (cut::Standing::Due, &[8][..]));
        assert!(cut::starts(row, set.held(), set.frontier.len(), false), "idle: the cut starts");

        // Its version installed, the cut took the label off: worked again.
        let returned = [issue(8, READY)];
        let tag = cut::Tag { version: cut::Version::parse("0.1.6").unwrap(), commit: "u".into() };
        let cut = cut::Seen { newest: Some(tag), past: 0, oldest_landing: None, ..due.clone() };
        let set = assemble(
            &Observed { issues: &returned, cut: Some(&cut), ..seen },
            Some(&me()),
            1000,
            &config(),
        );
        let frontier: Vec<u64> = set.frontier.iter().map(|d| d.number).collect();
        assert_eq!(frontier, [8]);
        assert!(set.cut.unwrap().waiting.is_empty());
    }

    #[test]
    fn an_unclaimed_issue_whose_kept_pr_nothing_live_owns_is_on_the_frontier() {
        let issues = [issue(1, READY), issue(2, READY), issue(3, READY), issue(4, READY)];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[pr(10, 1), pr(20, 2), pr(30, 3), pr(40, 4)],
            claims: &[claim(2, "peer")],
            heartbeats: &[("peer".into(), 1000)],
            closed: &[],
            gone: &[],
            // #1's, #2's and #3's PRs are the fleet's; #4's is somebody else's.
            kept: &[1, 2, 3],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &config());
        let frontier: Vec<u64> = set.frontier.iter().map(|d| d.number).collect();
        assert_eq!(frontier, [1, 3], "an escalation or a park kept its PR: it is taken up");
        let reasons: Vec<(u64, &str)> =
            set.excluded.iter().map(|e| (e.number, e.reason.as_str())).collect();
        assert_eq!(reasons, [(2, "already claimed"), (4, NOT_OURS)]);
    }

    #[test]
    fn an_escalated_issue_is_excluded_as_escalated_and_waits_for_a_human() {
        use crate::waiting::Back;
        let back = || Back::Ready { ready: "ready-for-agent".into(), escalate: "stuck".into() };
        let issues = [issue(2, &["bug"]), issue(3, &["stuck"]), issue(4, &["stuck"])];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[],
            // An escalation still under way: its claim is released last.
            claims: &[claim(4, "peer")],
            heartbeats: &[("peer".into(), 1000)],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[
                (4, Waits::Escalated(back()), Some(900)),
                (3, Waits::Escalated(back()), Some(400)),
                // One that closed since it was read.
                (9, Waits::Ended, None),
            ],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &config());
        let reasons: Vec<(u64, &str)> =
            set.excluded.iter().map(|e| (e.number, e.reason.as_str())).collect();
        assert_eq!(
            reasons,
            [
                (2, "no ready-for-agent label"),
                (3, "escalated: waits for the ready label"),
                (4, "no ready-for-agent label"),
            ]
        );
        let waiting = &set.waiting_on_human;
        assert_eq!(waiting.len(), 1, "{waiting:?}");
        assert_eq!((waiting[0].issue, waiting[0].since), (3, Some(400)));
        assert_eq!(waiting[0].title, "issue 3");
        assert_eq!(
            waiting[0].answer,
            "gh issue edit 3 --add-label ready-for-agent --remove-label stuck"
        );
    }

    #[test]
    fn with_a_lock_standing_the_frontier_is_the_lock_issues_and_nothing_else() {
        let issues = [
            issue(1, READY),
            issue(2, &["ready-for-agent", "fleet-lock"]),
            issue(3, &["bug"]),
            // A lock that is not ready is worked by nobody, and still stands.
            issue(4, &["fleet-lock"]),
            issue(5, READY),
            issue(6, READY),
        ];
        let no_flows = FlowLabels::default();
        let seen = |issues, claims| Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues,
            prs: &[],
            claims,
            heartbeats: &[],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &no_flows,
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let claims = [claim(5, "me"), claim(6, "me")];
        let set = assemble(&seen(&issues, &claims), Some(&me()), 1000, &config());
        assert_eq!(set.locks, [2, 4]);
        assert_eq!(set.frontier.iter().map(|d| d.number).collect::<Vec<_>>(), [2]);
        let reasons: Vec<(u64, &str)> =
            set.excluded.iter().map(|e| (e.number, e.reason.as_str())).collect();
        assert_eq!(
            reasons,
            [
                (1, "a fleet lock stands (#2, #4)"),
                (3, "no ready-for-agent label"),
                (4, "no ready-for-agent label"),
                (5, "already claimed"),
                (6, "already claimed"),
            ]
        );
        // Both slots are held by workers the lock leaves alone: the lock
        // issue does not wait for them.
        assert_eq!((set.left_alone(), set.bound(2)), (2, 4));
        assert!(set.locked_out(5) && !set.locked_out(2));
        // The last lock closed: everything is as it was.
        let open = [issues[0].clone(), issues[2].clone(), issues[4].clone(), issues[5].clone()];
        let after = assemble(&seen(&open, &claims), Some(&me()), 1000, &config());
        assert_eq!(after.locks, Vec::<u64>::new());
        assert_eq!(after.frontier.iter().map(|d| d.number).collect::<Vec<_>>(), [1]);
        assert_eq!(after.bound(2), 2);
    }

    #[test]
    fn in_a_bound_repo_a_lock_holds_back_what_a_worker_and_a_human_were_due() {
        use crate::position::Declared;
        use crate::role::due;
        use crate::stage_record::{Author, StageRecord, Stated};
        use crate::team::tests::{FEATURE, one};
        let team = one(FEATURE);
        let escalated = [StageRecord {
            stage: "plan".to_string(),
            stated: Stated::Declared(Declared::Escalate),
            by: Author::Worker {
                consult: false,
                claim: "c1".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            blocked_by: Default::default(),
            text: "stuck".to_string(),
        }];
        let issues = [
            issue(1, &["flow:feature"]),
            issue(2, &["flow:feature", "fleet-lock"]),
            issue(3, &["flow:feature"]),
        ];
        let of = |number: usize, records: &[StageRecord]| {
            (number as u64, due(&team, &issues[number - 1].labels, records, None))
        };
        let due = [of(1, &[]), of(2, &[]), of(3, &escalated)];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[],
            claims: &[],
            heartbeats: &[],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &due,
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &bound());
        let frontier: Vec<(u64, Option<&str>)> =
            set.frontier.iter().map(|d| (d.number, d.stage.as_deref())).collect();
        assert_eq!(frontier, [(2, Some("plan"))], "the lock follows its workflow like any other");
        assert_eq!(set.escalating, [], "a hand-over waits for the lock to end, too");
        let reason = "a fleet lock stands (#2)".to_string();
        assert_eq!(
            set.excluded,
            [1, 3].map(|number| Excluded {
                number,
                title: format!("issue {number}"),
                reason: reason.clone()
            })
        );
    }

    fn bound() -> Config {
        Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            team: Some("acme/team".into()),
            ..Settings::default()
        })
        .unwrap()
    }

    #[test]
    fn in_a_bound_repo_the_frontier_is_the_issues_a_worker_is_due_on_each_at_its_stage() {
        use crate::position::Declared;
        use crate::role::due;
        use crate::stage_record::{Author, StageRecord, Stated};
        use crate::team::tests::{FEATURE, one};
        let team = one(FEATURE);
        let record = |stage: &str, declared, claim: &str| StageRecord {
            stage: stage.to_string(),
            stated: Stated::Declared(declared),
            by: Author::Worker {
                consult: false,
                claim: claim.to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            blocked_by: Default::default(),
            text: "done".to_string(),
        };
        let planned = [record("plan", Declared::Pass, "c1")];
        let signed = [
            record("plan", Declared::Pass, "c1"),
            record("build", Declared::Pass, "c2"),
            record("review", Declared::Pass, "c3"),
            record("sign-off", Declared::Pass, "c4"),
        ];
        let parked = [&planned[..], &[record("build", Declared::Blocked, "c2")]].concat();
        let escalated = [record("plan", Declared::Escalate, "c1")];
        let ended = [record("plan", Declared::AlreadySatisfied, "c1")];
        let mut waiting = issue(6, &["flow:feature"]);
        waiting.open_blockers = vec![41];
        let issues = [
            issue(1, &["flow:feature"]),
            issue(2, READY),
            issue(3, &["flow:feature", "epic"]),
            issue(4, &["flow:chore"]),
            issue(5, &["flow:feature"]),
            waiting,
            issue(7, &["flow:feature"]),
            issue(8, &["flow:feature"]),
            issue(9, &["flow:feature"]),
            issue(10, &["flow:feature"]),
            issue(11, &["flow:feature"]),
            issue(12, &["flow:feature"]),
        ];
        // An open PR declares nothing: #5 has a stage to perform.
        let prs = [pr(50, 5), pr(110, 11)];
        let of = |number: u64, records: &[StageRecord]| {
            let carried = &issues.iter().find(|issue| issue.number == number).unwrap().labels;
            (number, due(&team, carried, records, closing_pr(&prs, number).map(|pr| pr.number)))
        };
        let due = [
            of(1, &[]),
            of(3, &[]),
            of(4, &[]),
            of(5, &planned),
            of(6, &parked),
            of(7, &parked),
            of(8, &escalated),
            of(9, &ended),
            of(10, &planned),
            of(11, &signed),
            of(12, &signed),
        ];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &prs,
            claims: &[claim(10, "peer")],
            heartbeats: &[("peer".into(), 1000)],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &due,
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[5, 6, 7, 8, 9, 10, 11, 12],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &bound());
        let frontier: Vec<(u64, Option<&str>)> =
            set.frontier.iter().map(|d| (d.number, d.stage.as_deref())).collect();
        // #7 returned from its park: it is at the stage that parked it.
        // #11's last stage passed to the landing train: its PR is to be joined.
        // #1, which no stage ever declared on, comes after what was begun.
        assert_eq!(
            frontier,
            [(5, Some("build")), (7, Some("build")), (11, Some("land")), (1, Some("plan"))]
        );
        let reasons = |rows: &[Excluded]| -> Vec<(u64, String)> {
            rows.iter().map(|e| (e.number, e.reason.clone())).collect()
        };
        assert_eq!(
            reasons(&set.escalating),
            [
                (4, "its flow label flow:chore names no workflow of the team".to_string()),
                (8, "the stage `plan` declared `escalate`, which it hands to a human".to_string()),
                (
                    12,
                    "its last stage passed it to the landing train, and no open PR closes it: \
                     there is nothing to join"
                        .to_string()
                ),
            ]
        );
        assert_eq!(
            reasons(&set.excluded),
            [
                (2, "no flow label".to_string()),
                (3, "epic label (epic)".to_string()),
                (6, "open blockers: #41".to_string()),
                (9, "its workflow has ended".to_string()),
                (10, "already claimed".to_string()),
            ]
        );

        // Under a claim of mine, a worker is due where the frontier would
        // hold the issue at a stage a role performs: not behind an epic
        // label or an open blocker, and never for the join.
        let claims = [claim(3, "me"), claim(5, "me"), claim(6, "me"), claim(7, "me")];
        let landing = [claim(11, "me")];
        let held = Observed { claims: &[&claims[..], &landing].concat(), ..seen };
        let mine = assemble(&held, Some(&me()), 1000, &bound()).mine;
        let due: Vec<(u64, Option<&str>)> =
            mine.iter().map(|mine| (mine.number, mine.due.as_deref())).collect();
        assert_eq!(due, [(3, None), (5, Some("build")), (6, None), (7, Some("build")), (11, None)]);
    }

    #[test]
    fn a_bound_repo_s_claim_is_at_a_stage_whatever_pr_is_open_until_a_landing_worker_holds_it() {
        let issues = [
            issue(1, &["flow:feature"]),
            issue(7, &["flow:feature"]),
            issue(9, &["flow:feature"]),
            issue(10, &["flow:feature"]),
        ];
        let claims =
            [claim(1, "me"), claim(6, "me"), claim(7, "me"), claim(8, "me"), claim(9, "me")];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[pr(70, 7), pr(80, 8), pr(90, 9)],
            claims: &[&claims[..], &[claim(10, "me")]].concat(),
            heartbeats: &[],
            closed: &[6],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            // #10's PR is gone: its landing worker has nothing to join.
            landing: &[8, 9, 10],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 1000, &bound());
        let mine: Vec<_> = set.mine.iter().map(|m| (m.number, m.status, m.pr)).collect();
        assert_eq!(
            mine,
            [
                (1, Status::AtStage, None),
                (6, Status::Closed, None),
                (7, Status::AtStage, Some(70)),
                (8, Status::Landing, Some(80)),
                (9, Status::Landing, Some(90)),
                (10, Status::AtStage, None),
            ]
        );
    }

    #[test]
    fn claims_are_partitioned_by_owner_and_liveness() {
        let issues = [issue(1, READY), issue(2, READY), issue(3, READY), issue(5, READY)];
        let nobody = Claim::read(5, "", "c5".into());
        let claims = [
            claim(1, "me"),
            claim(2, "live"),
            claim(3, "dead"),
            claim(4, "dead"),
            nobody,
            claim(6, "me"),
            claim(7, "me"),
            claim(8, "me"),
            claim(9, "me"),
        ];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &issues,
            prs: &[pr(70, 7), pr(80, 8)],
            claims: &claims,
            // my own heartbeat has expired: my claims stay mine
            heartbeats: &[
                ("live".into(), 10_000 - LEASE),
                ("dead".into(), 9_999 - LEASE),
                ("me".into(), 1),
            ],
            closed: &[4, 6],
            gone: &[],
            kept: &[],
            landed: &[9],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 10_000, &config());
        let mine: Vec<_> = set.mine.iter().map(|m| (m.number, m.status, m.pr)).collect();
        assert_eq!(
            mine,
            [
                (1, Status::NoPr, None),
                (6, Status::Closed, None),
                (7, Status::Landing, Some(70)),
                (8, Status::Landing, Some(80)),
                (9, Status::Landed, None),
            ]
        );
        assert_eq!(set.mine[0].title.as_deref(), Some("issue 1"));
        assert_eq!(set.mine[1].title, None);
        assert_eq!(set.peer_live.iter().map(|c| c.number).collect::<Vec<_>>(), [2]);
        assert_eq!(set.stale.iter().map(|c| c.number).collect::<Vec<_>>(), [3, 5]);
        assert_eq!(set.stale[1].instance, None, "a claim that names nobody is stale to everyone");
        assert_eq!(
            set.stale_closed.iter().map(|c| (c.number, c.sha.as_str())).collect::<Vec<_>>(),
            [(4, "c4")]
        );
    }

    #[test]
    fn a_claim_on_an_issue_that_is_gone_is_let_go_by_its_holder_and_unlocked_if_stale() {
        let claims = [claim(1, "me"), claim(2, "live"), claim(3, "dead"), claim(4, "me")];
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &[],
            prs: &[],
            claims: &claims,
            heartbeats: &[("live".into(), 10_000), ("dead".into(), 9_999 - LEASE)],
            closed: &[4],
            gone: &[1, 2, 3],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        let set = assemble(&seen, Some(&me()), 10_000, &config());
        let mine: Vec<_> = set.mine.iter().map(|m| (m.number, m.status, m.title.clone())).collect();
        assert_eq!(mine, [(1, Status::Gone, None), (4, Status::Closed, None)]);
        assert_eq!(set.peer_live.iter().map(|c| c.number).collect::<Vec<_>>(), [2]);
        assert_eq!(set.stale_closed.iter().map(|c| c.number).collect::<Vec<_>>(), [3]);
        assert_eq!(set.gone, [1, 2, 3]);
    }

    #[test]
    fn a_heartbeat_from_the_future_is_stale() {
        let seen = Observed {
            unworked_blockers: &[],
            handoffs: &[],
            issues: &[issue(1, READY)],
            prs: &[],
            claims: &[claim(1, "peer")],
            heartbeats: &[("peer".into(), 5000)],
            closed: &[],
            gone: &[],
            kept: &[],
            landed: &[],
            declared: &[],
            idle: &[],
            due: &[],
            flows: &FlowLabels::default(),
            landing: &[],
            begun: &[],
            overlapping: &[],
            patrols: &[],
            waiting: &[],
            landings: &[],
            slow_gate: &Reported::NONE,
            tracker: &Tracker::Own,
            cut: None,
        };
        assert_eq!(assemble(&seen, None, 1000, &config()).stale.len(), 1);
    }

    #[test]
    fn the_same_state_gives_the_same_working_set_in_any_order() {
        let mut issues = vec![issue(3, READY), issue(1, READY), issue(2, READY), issue(8, &[])];
        let mut prs = vec![pr(21, 2), pr(20, 2), pr(22, 8)];
        let mut claims = vec![claim(3, "me"), claim(8, "peer"), claim(9, "gone")];
        let mut beats = vec![("peer".to_string(), 990u64), ("me".to_string(), 1000)];
        let config = config();
        let build =
            |issues: &[Issue], prs: &[PullRequest], claims: &[Claim], beats: &[(String, u64)]| {
                let seen = Observed {
                    unworked_blockers: &[],
                    handoffs: &[],
                    issues,
                    prs,
                    claims,
                    heartbeats: beats,
                    closed: &[9],
                    gone: &[],
                    kept: &[],
                    landed: &[],
                    declared: &[],
                    idle: &[],
                    due: &[],
                    flows: &FlowLabels::default(),
                    landing: &[],
                    begun: &[],
                    overlapping: &[],
                    patrols: &[],
                    waiting: &[],
                    landings: &[],
                    slow_gate: &Reported::NONE,
                    tracker: &Tracker::Own,
                    cut: None,
                };
                assemble(&seen, Some(&me()), 1000, &config)
            };
        let first = build(&issues, &prs, &claims, &beats);
        for _ in 0..6 {
            issues.rotate_left(1);
            prs.reverse();
            claims.rotate_left(1);
            beats.reverse();
            assert_eq!(build(&issues, &prs, &claims, &beats), first);
        }
        assert_eq!(closing_pr(&prs, 2).unwrap().number, 21, "the latest PR is the live one");
    }
}

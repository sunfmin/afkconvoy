//! The panel: the one page that shows a human where every fleet of a host
//! stands (docs/adr/0023) — the host first, once; then what waits for a
//! human across its repos, with the fleet instances that are broken; then
//! each repo: its workflows as lines of stages, the issues at work, queued
//! and held back at each, and the landing train they end on. Like a status
//! board it is rendered from what the fleet read and never read back.
//!
//! A `Panel` is one repo's part, as one tick or one rebuild shows it, and a
//! `Page` is the whole: the `Host` and each checkout. `Panel::show` decides
//! what goes where and `render` puts a page into HTML, one rendering for
//! each part, whoever made the page; both are pure. `read` asks orca after
//! each worker's state, `Host::read` reads the host, and `printed` makes the
//! page `afkconvoy panel` prints, from one rebuild of each checkout.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::patrol::{self, Standing};
use afkconvoy_core::plan::{Excluded, Status, WorkingSet};
use afkconvoy_core::position::Position;
use afkconvoy_core::team::{LAND, Performer, Team, Work, Workflow};
use afkconvoy_core::tick::{Outcome, TickStep};
use afkconvoy_core::worker::WorkerState;
use anyhow::Result;

use crate::admission::{Admission, Readings};
use crate::checkout_list;
use crate::claims::Me;
use crate::fleet::Fleet;
use crate::fleet_report::FleetReport;
use crate::git::Repo;
use crate::host_runner::SharedNow;
use crate::hot::{HotNow, held_said};
use crate::kit::Kit;
use crate::log::Log;
use crate::orca::{Source, Worktree};
use crate::stage::Flows;
use crate::{doctor, host_runner, host_settings};

/// The stage of an unbound repo's one lane a worker writes its PR at.
const WORK: &str = "work";
/// The name of an unbound repo's one lane.
const BACKLOG: &str = "backlog";

/// One issue, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub number: u64,
    pub title: String,
    pub pr: Option<u64>,
    /// The other fleet instance that holds its claim, when one does.
    pub peer: Option<String>,
    /// What its worker is doing: known only of a claim of this instance.
    pub worker: Option<WorkerState>,
    /// Why it is where it is, or what the tick just did with it.
    pub note: Option<String>,
    /// The source that explains why this issue waits for a human.
    pub url: Option<String>,
}

impl Card {
    fn new(number: u64, title: &str) -> Card {
        Card {
            number,
            title: title.to_string(),
            pr: None,
            peer: None,
            worker: None,
            note: None,
            url: None,
        }
    }

    fn noted(mut self, note: &str) -> Card {
        self.note = Some(note.to_string());
        self
    }
}

/// One stage of a lane, with the issues at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// Who performs it: a role's skill, or a human.
    pub by: String,
    /// Claimed: a worker was started on each.
    pub at_work: Vec<Card>,
    /// On the frontier: each waits for a free slot.
    pub queued: Vec<Card>,
    /// At this stage, and not the fleet's to start: each says why.
    pub held: Vec<Card>,
    /// Whether a human performs it: what is held here waits for them.
    pub human: bool,
}

impl Node {
    fn new(name: &str, by: String) -> Node {
        let (name, human) = (name.to_string(), false);
        Node { name, by, at_work: vec![], queued: vec![], held: vec![], human }
    }
}

/// One workflow, drawn as the line of its stages. An unbound repo has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub name: String,
    pub nodes: Vec<Node>,
}

/// The tick a panel was made after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub count: u64,
    /// What it did, as `run` printed it.
    pub line: String,
    /// When it ended, in epoch seconds, and the seconds slept before the next.
    pub ended: u64,
    pub sleep: u64,
}

/// One repo's part of the page: what one tick of its fleet instance, or one
/// rebuild, shows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub repo: String,
    pub base_branch: String,
    /// The repo its fleet reads and writes its issues in, where that is not
    /// its own (docs/adr/0027).
    pub tracker: Option<String>,
    /// The fleet instance whose view this is, and its host; `None` of nobody's:
    /// every claim is then another instance's.
    pub who: Option<(String, String)>,
    /// `None` of a part no tick made: it shows a rebuild.
    pub tick: Option<Tick>,
    /// The claims held that take a slot, of the `bound` the fleet may hold.
    pub held: usize,
    pub bound: usize,
    /// The claims held whose worker declared a verdict not acted on yet: they
    /// take no slot, and are counted apart.
    pub declared: usize,
    /// The issues a worker can be started on now that the tick left: those
    /// queued, and those at no stage the page draws.
    pub frontier: usize,
    /// Whether its fleet instance has nothing to do now (GLOSSARY: Idle).
    pub idle: bool,
    pub locks: Vec<u64>,
    pub lanes: Vec<Lane>,
    /// The issues that wait for a human, each with why and the line that
    /// answers it: the working set's `waiting_on_human`, as `doctor` gives it.
    /// The page lists them across every repo.
    pub to_human: Vec<Card>,
    /// Every other open issue, with why the fleet leaves it.
    pub off: Vec<Card>,
    /// The patrols the repo names, each as it stands.
    pub patrols: Vec<patrol::Row>,
    /// Where the repo's cut stands, and the last one, in words: `None` where
    /// it turns cutting off and no cut is going.
    pub cut: Option<String>,
    /// When the part was made, in epoch seconds: what a patrol's wait is
    /// counted from.
    pub now: u64,
    /// Its workers hot as the tick ended — or, of a rebuild, as its tick log
    /// has them. The page lists them with the host.
    pub hot: Vec<Hot>,
    pub casting: Vec<afkconvoy_core::casting::ModelStatus>,
    /// Each role no worker of which can be started on this host, with why
    /// (`casting::nothing_safe`): its stages wait while it lasts.
    pub nothing_safe: Vec<(String, String)>,
    /// What each Codex session was actually started with, including sources.
    pub codex_workers: Vec<serde_json::Value>,
    /// The same authoritative candidate interpretation doctor reads.
    pub candidates: Vec<serde_json::Value>,
    /// Each Jira delivery, as one line: its reference, identity and handoff detail.
    pub deliveries: Vec<String>,
}

/// A worker that is hot, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hot {
    /// `#7`, or the name of its worktree.
    pub worker: String,
    /// When it turned hot.
    pub since: u64,
    /// Why, under the limits in force as it did, and what it held at the
    /// most, in words.
    pub why: String,
    pub held: String,
}

impl From<&HotNow> for Hot {
    fn from(hot: &HotNow) -> Hot {
        let (worker, since) = (hot.worker.clone(), hot.episode.since);
        Hot { worker, since, why: hot.why.clone(), held: held_said(&hot.episode) }
    }
}

/// The whole page: the host, and each checkout on its list.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub host: Host,
    /// Whether the host runner serves it, made anew after every tick: else
    /// `afkconvoy panel` printed it, from one rebuild of each checkout.
    pub live: bool,
    /// In the order of their roots.
    pub checkouts: Vec<Checkout>,
}

/// One checkout, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// Its root.
    pub dir: PathBuf,
    pub part: Part,
    /// Its fleet instance broken: the host runner starts it again after a
    /// backoff. What it showed before is shown still.
    pub broken: Option<Broken>,
}

/// What the page shows of a checkout's repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Its part, as its fleet instance's last tick, or a rebuild, made it.
    Shown(Box<Panel>),
    /// Its fleet instance's first tick has not ended.
    Starting,
    /// A rebuild of it failed, as this says.
    Unread(String),
}

/// A fleet instance that is broken: why it last broke, since when, and when
/// the host runner starts it again — `None` where no host runner runs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    pub why: String,
    pub since: u64,
    pub next: Option<u64>,
}

impl Checkout {
    /// The name its part goes by: its repo's, else its root's last name.
    pub fn name(&self) -> String {
        match &self.part {
            Part::Shown(panel) => panel.repo.clone(),
            Part::Starting | Part::Unread(_) => {
                let name = self.dir.file_name().map(|name| name.to_string_lossy().into_owned());
                name.unwrap_or_default()
            }
        }
    }

    /// Its part, where one was made.
    fn shown(&self) -> Option<&Panel> {
        match &self.part {
            Part::Shown(panel) => Some(panel),
            Part::Starting | Part::Unread(_) => None,
        }
    }
}

/// The host, as the page shows it once, above every repo.
#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub name: String,
    /// When it was read, in epoch seconds.
    pub now: u64,
    /// Admission's reading of the host now; why it cannot be read.
    pub admission: Result<Admission, String>,
    /// What its host runner says of the exclusive queue; why it cannot.
    pub exclusive: Result<crate::queue::Said, String>,
    /// How many commands may run at once in its worker lanes, fast lane and
    /// slow lanes.
    pub lanes: afkconvoy_core::exclusive::Lanes,
    /// Its host kit, every harness in it; why it cannot be read.
    pub kit: Result<Kit, String>,
    /// Worker health; why it cannot be read.
    pub worker_health: Result<crate::health::Health, String>,
    pub watch: serde_json::Value,
    pub asking: serde_json::Value,
    /// The harnesses paused on this host, since when and why: no worker is
    /// started on one until a probe finds it working.
    pub paused: Vec<afkconvoy_core::casting::Paused>,
}

impl Host {
    /// The host as it is at `now`, read soft: what cannot be read is said
    /// so, or shows nothing. Admission reads the disk under `dir`; `shared`
    /// is what its host runner holds for its fleets, `None` where none runs;
    /// `readings` the look's reading of the host.
    pub fn read(
        dir: &Path,
        now: u64,
        shared: Result<Option<SharedNow>, String>,
        readings: &Readings,
    ) -> Host {
        let settings = host_settings::of_host().unwrap_or_default();
        let (exclusive, watch) = match shared {
            Ok(Some(shared)) => {
                (Ok(Some((shared.exclusive, shared.exclusive_paused))), shared.watch)
            }
            Ok(None) => (Ok(None), None),
            Err(error) => (Err(error), None),
        };
        Host {
            name: crate::this_host(),
            now,
            admission: Admission::read_from(dir, readings).map_err(|error| format!("{error:#}")),
            exclusive,
            lanes: settings.lanes(),
            kit: crate::kit::load().map_err(|error| format!("{error:#}")),
            worker_health: crate::health::show(),
            watch: crate::host_watch::json(watch.as_ref(), false),
            asking: crate::host_asking::json(),
            paused: crate::paused::read(),
        }
    }
}

/// What a panel is made from: one rebuild, and the pass made over it.
pub struct Seen<'a> {
    pub set: &'a WorkingSet,
    /// Whether the repo names a team: an unbound one has the one fixed lane.
    pub bound_repo: bool,
    /// The team; none where no team was read, or it did not load.
    pub team: Option<&'a Team>,
    pub positions: &'a [(u64, &'a Workflow, Position<'a>)],
    /// The state of the worker of each claim of mine that was asked after.
    pub workers: &'a [(u64, WorkerState)],
    /// The steps the tick carried out; none where no tick was made.
    pub did: &'a [(TickStep, Outcome)],
}

fn lane_of(team: &Team, workflow: &Workflow) -> Lane {
    let said = |work: &Work| format!("{}: {}", team.role_of(work).name, work.skill);
    let mut nodes: Vec<Node> = vec![];
    for stage in workflow.stages() {
        let by = match &stage.by {
            Performer::Human => "a human".to_string(),
            Performer::Role(work) => said(work),
        };
        let human = stage.by == Performer::Human;
        nodes.push(Node { human, ..Node::new(&stage.name, by) });
    }
    if let Some(land) = &workflow.land {
        nodes.push(Node::new(LAND, said(land)));
    }
    Lane { name: workflow.name.clone(), nodes }
}

fn lanes(seen: &Seen) -> Vec<Lane> {
    if seen.bound_repo {
        let Some(team) = seen.team else { return vec![] };
        return team.workflows.iter().map(|workflow| lane_of(team, workflow)).collect();
    }
    let nodes = vec![
        Node::new(WORK, "one worker writes the PR".to_string()),
        Node::new(LAND, "the same worker lands in a turn".to_string()),
    ];
    vec![Lane { name: BACKLOG.to_string(), nodes }]
}

/// The lane and the stage issue `number` is at; no stage once it left them.
fn place(seen: &Seen, number: u64) -> Option<(String, Option<String>)> {
    if !seen.bound_repo {
        return Some((BACKLOG.to_string(), Some(WORK.to_string())));
    }
    let (_, workflow, position) = seen.positions.iter().find(|(at, ..)| *at == number)?;
    let stage = match position {
        at if at.landing() => Some(LAND.to_string()),
        _ => position.stage().map(|stage| stage.name.clone()),
    };
    Some((workflow.name.clone(), stage))
}

/// What the tick's `step` says of the issue it was about, once it ended in
/// `outcome`; `None` for a step that shows on no card.
fn said(step: TickStep, outcome: Outcome) -> Option<(u64, &'static str)> {
    let issue = match step {
        TickStep::Judge { issue, .. }
        | TickStep::Watch { issue, .. }
        | TickStep::Advance { issue }
        | TickStep::Escalate { issue }
        | TickStep::Reclaim { issue }
        | TickStep::Dispatch { issue } => issue,
        _ => return None,
    };
    let says = match (step, outcome) {
        (_, Outcome::Failed) => "a step of this tick failed on it",
        (_, Outcome::Nudged) => "nudged this tick",
        (_, Outcome::Waiting(_)) => "its worker waits on its own background work",
        (_, Outcome::Restarted) => "its worker was replaced this tick",
        (_, Outcome::Continued) => "a worker was started on it again this tick",
        (_, Outcome::Asked) => "its turn was asked for again this tick",
        (TickStep::Dispatch { .. }, Outcome::Done) => "started this tick",
        (TickStep::Dispatch { .. }, Outcome::Blocked) => "skipped this tick: a blocker is open",
        (TickStep::Reclaim { .. }, Outcome::Done) => "taken from a silent peer this tick",
        (TickStep::Escalate { .. }, Outcome::Done) => "handed over this tick",
        (_, Outcome::Done | Outcome::HandedOn) => "its claim ended this tick",
        _ => return None,
    };
    Some((issue, says))
}

impl Panel {
    /// A panel with nothing on it yet, for the fleet instance `me` in `fleet`.
    pub fn empty(fleet: &Fleet, me: Option<&Me>) -> Panel {
        let dir = fleet.repo.main_checkout().unwrap_or_else(|_| fleet.repo.dir.clone());
        let name = dir.file_name().map(|name| name.to_string_lossy().into_owned());
        Panel {
            repo: name.unwrap_or_default(),
            base_branch: fleet.config.base_branch.clone(),
            tracker: fleet.gh.tracker.named().map(|tracker| tracker.to_string()),
            who: me.map(|me| (me.instance.as_str().to_string(), me.host.clone())),
            tick: None,
            held: 0,
            bound: fleet.config.concurrency(),
            declared: 0,
            frontier: 0,
            idle: false,
            locks: vec![],
            lanes: vec![],
            to_human: vec![],
            off: vec![],
            patrols: vec![],
            cut: None,
            now: fleet.now,
            hot: vec![],
            casting: vec![],
            nothing_safe: vec![],
            codex_workers: vec![],
            candidates: vec![],
            deliveries: vec![],
        }
    }

    /// Puts every open issue of `seen` where the page shows it: at work or
    /// queued at its stage, held back there, on the train, or left — and,
    /// whatever stage it is held at, with a human when it waits for one.
    pub fn show(mut self, seen: &Seen) -> Panel {
        let set = seen.set;
        self.lanes = lanes(seen);
        self.locks = set.locks.clone();
        self.casting = set.casting.clone();
        self.patrols = set.patrols.clone();
        let mut put = Put { panel: &mut self, seen };

        for mine in &set.mine {
            let mut card = Card::new(mine.number, mine.title.as_deref().unwrap_or_default());
            card.pr = mine.pr;
            card.worker =
                seen.workers.iter().find(|(issue, _)| *issue == mine.number).map(|(_, w)| *w);
            match mine.status {
                // Settled or let go by the same tick: there is nothing left of them to show.
                Status::Landed | Status::Closed | Status::Gone => {}
                Status::Landing => put.at(Some(LAND), card, List::AtWork),
                Status::NoPr | Status::AtStage => put.at(None, card, List::AtWork),
            }
        }
        for (peer, stale) in
            set.peer_live.iter().map(|p| (p, false)).chain(set.stale.iter().map(|p| (p, true)))
        {
            let title = title_of(set, peer.number);
            let mut card = Card::new(peer.number, &title);
            card.peer = Some(peer.instance.clone().unwrap_or_else(|| "nobody named".to_string()));
            if stale {
                card = card.noted("its fleet instance went silent: this one takes it");
            }
            put.at(None, card, List::AtWork);
        }
        for issue in &set.frontier {
            let mut card = Card::new(issue.number, &issue.title);
            // An unbound repo's says what its worker would run, and why.
            if let Some(cast) = &issue.cast {
                let on =
                    cast.model.as_deref().map_or(String::new(), |model| format!(" on {model}"));
                card = card.noted(&format!("runs {}{on}: {}", cast.harness.key(), cast.why.said()));
            }
            put.at(issue.stage.as_deref(), card, List::Queued);
        }
        for waiting in &set.waiting_on_human {
            let note = format!("{} — {}", waiting.why, waiting.answer);
            let mut card = Card::new(waiting.issue, &waiting.title).noted(&note);
            card.url = waiting.url.clone();
            put.panel.to_human.push(card);
        }
        for Excluded { number, title, reason } in &set.excluded {
            if set.mine.iter().any(|mine| mine.number == *number)
                || set.peer_live.iter().chain(&set.stale).any(|peer| peer.number == *number)
            {
                continue;
            }
            // One that waits for a human at no stage is with them, and nowhere else.
            let waits = set.waiting_on_human.iter().any(|waiting| waiting.issue == *number);
            if waits && !(seen.bound_repo && put.at_a_node(*number)) {
                continue;
            }
            let card = Card::new(*number, title).noted(reason);
            // Only a bound repo's issue is at a stage before a worker has it.
            if seen.bound_repo {
                put.at(None, card, List::Held);
            } else {
                put.panel.off.push(card);
            }
        }
        for (step, outcome) in seen.did {
            if let Some((issue, says)) = said(*step, *outcome) {
                self.tell(issue, says, matches!(step, TickStep::Dispatch { .. }));
            }
        }
        self
    }

    /// Notes on the card of `issue` what the tick did with it; one the tick
    /// `started` a worker on is at work from here on, and its claim is mine.
    fn tell(&mut self, issue: u64, says: &str, started: bool) {
        for node in self.lanes.iter_mut().flat_map(|lane| &mut lane.nodes) {
            if started && let Some(at) = node.queued.iter().position(|card| card.number == issue) {
                let card = node.queued.remove(at);
                node.at_work.push(card);
            }
            let cards = node.at_work.iter_mut().chain(&mut node.queued).chain(&mut node.held);
            for card in cards.filter(|card| card.number == issue) {
                card.note = Some(says.to_string());
                if started {
                    card.peer = None;
                }
            }
        }
        for card in self.to_human.iter_mut().filter(|card| card.number == issue) {
            card.note = Some(format!("{} — {says}", card.note.as_deref().unwrap_or_default()));
        }
    }

    /// How many issues wait for a human.
    pub fn with_a_human(&self) -> usize {
        self.to_human.len()
    }

    /// How many issues wait for a free slot.
    pub fn queued(&self) -> usize {
        self.lanes.iter().flat_map(|lane| &lane.nodes).map(|node| node.queued.len()).sum()
    }
}

/// The title of an open issue the set names without one: a peer's claim.
fn title_of(set: &WorkingSet, number: u64) -> String {
    let excluded = set.excluded.iter().find(|excluded| excluded.number == number);
    excluded.map(|excluded| excluded.title.clone()).unwrap_or_default()
}

/// The list of a node a card goes on.
#[derive(Clone, Copy)]
enum List {
    AtWork,
    Queued,
    Held,
}

/// Where a card goes, as `Panel::show` fills a panel in.
struct Put<'p, 'a> {
    panel: &'p mut Panel,
    seen: &'p Seen<'a>,
}

impl Put<'_, '_> {
    /// Whether the issue `number` is at a stage the page draws.
    fn at_a_node(&self, number: u64) -> bool {
        let placed = place(self.seen, number);
        placed.is_some_and(|(lane, at)| {
            let lane = self.panel.lanes.iter().find(|found| found.name == lane);
            at.zip(lane).is_some_and(|(at, lane)| lane.nodes.iter().any(|node| node.name == at))
        })
    }

    /// Puts `card` on the list `list` picks of the node its issue is at — the
    /// node named `stage`, when the caller knows it — or, for an issue at no
    /// node the page draws, among the issues the fleet leaves.
    fn at(&mut self, stage: Option<&str>, card: Card, list: List) {
        let placed = place(self.seen, card.number);
        let node = placed.and_then(|(lane, at)| {
            let stage = stage.map(str::to_string).or(at)?;
            let lane = self.panel.lanes.iter_mut().find(|found| found.name == lane)?;
            lane.nodes.iter_mut().find(|node| node.name == stage)
        });
        match node {
            Some(node) => match list {
                List::AtWork => node.at_work.push(card),
                List::Queued => node.queued.push(card),
                List::Held => node.held.push(card),
            },
            None if card.note.is_some() => self.panel.off.push(card),
            None => self.panel.off.push(card.noted("claimed, and at no stage of a workflow")),
        }
    }
}

/// The panel of the fleet instance `me` — `None`: of nobody — after a rebuild read `set` and
/// `flows`, and a tick — if one was made — carried out `did`; its tracker, its
/// bound, its deliveries and its roles that can run nothing safely as `report`
/// has them. Asks orca after the worker of every claim of mine at work; one
/// orca cannot speak of is shown with no state rather than failing the page.
pub fn read(
    fleet: &Fleet,
    me: Option<&Me>,
    set: &WorkingSet,
    flows: &Flows,
    did: &[(TickStep, Outcome)],
    report: &FleetReport,
) -> Panel {
    let source = Source::of(&fleet.repo).ok();
    let state = |issue: u64| {
        Worktree::worker_of(source.as_ref()?, issue, fleet.now).ok().map(|(_, state)| state)
    };
    let at_work = |status| matches!(status, Status::NoPr | Status::AtStage | Status::Landing);
    let mut workers = vec![];
    for mine in set.mine.iter().filter(|mine| at_work(mine.status)) {
        if let Some(state) = state(mine.number) {
            workers.push((mine.number, state));
        }
    }
    let seen = Seen {
        set,
        bound_repo: fleet.config.bound.is_some(),
        team: flows.team().ok(),
        positions: &flows.positions(),
        workers: &workers,
        did,
    };
    let mut panel = Panel::empty(fleet, me).show(&seen);
    panel.codex_workers = crate::codex_launch::workers(&fleet.repo);
    let prefix = fleet.repo.layout().under("candidates/");
    panel.candidates = crate::candidates::view(
        &fleet.repo,
        &fleet.config,
        &fleet.records_at_open(&format!("{prefix}*")),
    )
    .ok()
    .and_then(|rows| rows.as_array().cloned())
    .unwrap_or_default();
    panel.tracker = report.tracker_named();
    panel.deliveries = match &report.deliveries {
        Some(Ok(rows)) => rows.iter().map(crate::delivery::panel_line).collect(),
        Some(Err(error)) => vec![format!("deliveries could not be read: {error}")],
        None => vec![],
    };
    panel.declared = set.slotless();
    panel.held = set.mine.len() - panel.declared;
    let concurrency = report.concurrency_in_force().unwrap_or(fleet.config.concurrency());
    panel.bound = set.bound(concurrency) - panel.declared;
    panel.frontier = set.frontier.len();
    panel.idle = afkconvoy_core::tick::idle(set.held(), panel.frontier) && !set.cut_running();
    panel.cut = set.cut.as_ref().and_then(|row| crate::cut::says(&fleet.repo, row));
    // Soft: a team or a host kit that cannot be read names no role.
    panel.nothing_safe = report.nothing_safe().to_vec();
    panel
}

/// The page `afkconvoy panel` prints: one rebuild of each checkout on this
/// host's checkout list, or of `checkout` alone where it was `named` or the
/// list holds none, with the host as it is at `now`. Each is seen as the
/// fleet instance `instance` of `host` — where none is named, as the one its
/// tick log on this host names last: the one the host runner holds, or
/// held — with the workers hot and the break its tick log has. A rebuild
/// that fails is an error of a checkout alone, and of the list is said in
/// that checkout's part, the others shown.
pub fn printed(
    checkout: Result<Repo>,
    named: bool,
    instance: Option<&InstanceId>,
    host: &str,
    now: u64,
) -> Result<Page> {
    let mut listed = if named { vec![] } else { checkout_list::List::of_host()?.read()?.checkouts };
    listed.sort_by(|one, other| one.checkout.cmp(&other.checkout));
    let runs = host_runner::runs();
    // The page is one look: every checkout's report reads the host once.
    let readings = Readings::new();
    let checkouts = match listed.is_empty() {
        true => {
            let repo = checkout?;
            let entries = logged(&repo);
            let part =
                Part::Shown(Box::new(rebuilt(&repo, &entries, instance, host, now, &readings)?));
            let broken = broken_in(&entries, host, runs);
            vec![Checkout { dir: repo.dir, part, broken }]
        }
        false => listed
            .into_iter()
            .map(|entry| {
                let repo = Repo::at(&entry.checkout);
                let entries = repo.as_ref().map(logged).unwrap_or_default();
                let part =
                    repo.and_then(|repo| rebuilt(&repo, &entries, instance, host, now, &readings));
                let part = part.map_or_else(
                    |error| Part::Unread(format!("{error:#}")),
                    |panel| Part::Shown(Box::new(panel)),
                );
                let broken = broken_in(&entries, host, runs);
                Checkout { dir: entry.checkout, part, broken }
            })
            .collect(),
    };
    let dir = checkouts.first().map_or_else(|| PathBuf::from("/"), |first| first.dir.clone());
    // Soft: what the host runner holds for its fleets, where it cannot be read, says why.
    let shared = host_runner::shared().map_err(|error| format!("{error:#}"));
    Ok(Page { host: Host::read(&dir, now, shared, &readings), live: false, checkouts })
}

/// What the tick log of `repo` on this host holds; nothing where it cannot
/// be read.
fn logged(repo: &Repo) -> Vec<crate::log::Entry> {
    Log::of(repo).ok().and_then(|log| log.read()).unwrap_or_default()
}

/// The break of the fleet instance of `host` its tick log `entries` hold,
/// where it is broken; `runs`: whether a host runner runs to start it again.
fn broken_in(entries: &[crate::log::Entry], host: &str, runs: bool) -> Option<Broken> {
    let broken = doctor::read_broken(entries, host, runs)?;
    Some(Broken { why: broken.why, since: broken.since, next: broken.next })
}

/// The part of `repo` one rebuild shows, seen as `instance` of `host` or,
/// where none is named, as the fleet instance its tick log `entries` name
/// last; with the workers hot those entries have.
fn rebuilt(
    repo: &Repo,
    entries: &[crate::log::Entry],
    instance: Option<&InstanceId>,
    host: &str,
    now: u64,
    readings: &Readings,
) -> Result<Panel> {
    let (logged, ..) = doctor::read_log(entries, host);
    let logged = logged.and_then(|logged| InstanceId::parse(&logged).ok());
    let instance = instance.cloned().or(logged);
    let me = instance.map(|instance| Me { instance, host: host.to_string(), now });
    let (mut fleet, open) = Fleet::read(repo, now)?;
    let (set, flows) = fleet.rebuild(&open, me.as_ref().map(|me| &me.instance))?;
    let report = FleetReport::read(&fleet, &flows, &set, readings);
    let mut panel = read(&fleet, me.as_ref(), &set, &flows, &[], &report);
    panel.hot = doctor::read_hot(entries, host)
        .into_iter()
        .map(|hot| Hot { worker: hot.worker, since: hot.since, why: hot.why, held: hot.held })
        .collect();
    Ok(panel)
}

const STYLE: &str = include_str!("panel.css");
/// What keeps an open page fresh: `serve` hands it out beside the page.
pub const LIVE_SCRIPT: &str = include_str!("panel.js");

/// `text` with what HTML would read as markup spelled out.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A length of time in words short enough for a card: `40s`, `12m`, `3h 5m`.
pub(crate) fn lasted(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// How many of a thing, in words: `1 issue`, `3 issues`.
fn count(many: usize, one: &str) -> String {
    format!("{many} {one}{}", if many == 1 { "" } else { "s" })
}

/// The card of `card`, labelled with the name of its `repo` where the list
/// it is on spans repos.
fn card(out: &mut String, card: &Card, repo: Option<&str>) {
    let (class, worker) = match card.worker {
        Some(WorkerState::Busy) => ("busy", Some("busy".to_string())),
        Some(WorkerState::Idle { quiet: Some(quiet) }) => {
            ("stopped", Some(format!("stopped {} ago", lasted(quiet))))
        }
        Some(WorkerState::Idle { quiet: None }) => ("stopped", Some("stopped".to_string())),
        Some(WorkerState::Gone) => ("gone", Some("no worker".to_string())),
        None => ("", None),
    };
    let repo = repo.map(|repo| format!("<span class=\"repo\">{}</span> ", esc(repo)));
    let _ = write!(
        out,
        "<li class=\"card {class}\" data-issue=\"{n}\">{}<span class=\"num\">#{n}</span> \
         <span class=\"title\">{}</span>",
        repo.unwrap_or_default(),
        esc(&card.title),
        n = card.number,
    );
    let mut meta: Vec<String> = vec![];
    if let Some(worker) = worker {
        meta.push(format!("<span class=\"worker\">{worker}</span>"));
    }
    if let Some(peer) = &card.peer {
        meta.push(format!("<span class=\"peer\">held by {}</span>", esc(peer)));
    }
    if let Some(pr) = card.pr {
        meta.push(format!("<span class=\"pr\">PR #{pr}</span>"));
    }
    if let Some(note) = &card.note {
        meta.push(format!("<span class=\"note\">{}</span>", esc(note)));
    }
    if let Some(url) = &card.url {
        meta.push(format!("<a href=\"{}\">Reason</a>", esc(url)));
    }
    if !meta.is_empty() {
        let _ = write!(out, "<span class=\"meta\">{}</span>", meta.join(""));
    }
    out.push_str("</li>");
}

fn cards(out: &mut String, class: &str, heading: Option<&str>, list: &[Card]) {
    if list.is_empty() {
        return;
    }
    if let Some(heading) = heading {
        let _ = write!(out, "<h4>{heading} <span>{}</span></h4>", list.len());
    }
    let _ = write!(out, "<ul class=\"cards {class}\">");
    for one in list {
        card(out, one, None);
    }
    out.push_str("</ul>");
}

fn node(out: &mut String, node: &Node) {
    let load = match (node.at_work.len(), node.queued.len()) {
        (0, 0) if node.human && !node.held.is_empty() => "asking",
        (0, 0) => "idle",
        (0, _) => "waiting",
        _ => "working",
    };
    let held = if node.human { "With a human" } else { "Held back" };
    let mut loads: Vec<String> = vec![];
    for (many, what) in [
        (node.at_work.len(), "at work"),
        (node.queued.len(), "queued"),
        (node.held.len(), if node.human { "with a human" } else { "held back" }),
    ] {
        if many > 0 {
            loads.push(format!("<span><b>{many}</b> {what}</span>"));
        }
    }
    if loads.is_empty() {
        loads.push("<span>nothing here</span>".to_string());
    }
    let _ = write!(
        out,
        "<li class=\"node {load}\" data-stage=\"{name}\"><div class=\"stop\"><span>{}</span></div>\
         <h3>{name}</h3><p class=\"by\">{}</p><p class=\"load\">{}</p>",
        node.at_work.len(),
        esc(&node.by),
        loads.join(""),
        name = esc(&node.name),
    );
    cards(out, "at-work", None, &node.at_work);
    cards(out, "queued", Some("Queued"), &node.queued);
    cards(out, "held", Some(held), &node.held);
    out.push_str("</li>");
}

/// What holds the patrol `row` back, or what it is doing, at `now`, in words.
fn patrol_says(row: &patrol::Row, now: u64) -> String {
    match row.standing {
        Standing::Due if row.asked.is_some() => {
            "due: a human asked for it, and the next tick runs it".to_string()
        }
        Standing::Due => "due: the next tick runs it".to_string(),
        Standing::NotDue => match row.last_run {
            Some(last) => format!(
                "not due: it ran {} ago, and runs every {}",
                lasted(now.saturating_sub(last)),
                lasted(row.every)
            ),
            None => "not due".to_string(),
        },
        Standing::Spent => {
            let until = row.until.unwrap_or(now);
            format!(
                "held by max_runs_a_day: {} runs in the last 24h, of {} — until {} UTC, in {}",
                row.runs_in_day.unwrap_or_default(),
                row.max_runs_a_day.unwrap_or_default(),
                clock(until),
                lasted(until.saturating_sub(now)),
            )
        }
        Standing::Landed => {
            let until = row.until.unwrap_or(now);
            format!(
                "held by max_landed_a_day: {} completed issues in the last 24h, of {} — until {} UTC, in {}",
                row.landed_in_day.unwrap_or_default(),
                row.max_landed_a_day.unwrap_or_default(),
                clock(until),
                lasted(until.saturating_sub(now)),
            )
        }
        Standing::Locked => "held: a fleet lock stands".to_string(),
        Standing::Full => {
            format!("held by max_open: {} open issues it opened, of {}", row.open, row.max_open)
        }
        Standing::Failed => match row.failure {
            Some(failure) => format!("held: its last run failed, and #{failure} says so"),
            None => "held: its last run failed".to_string(),
        },
        Standing::Running => "running".to_string(),
        Standing::PeerRunning => "running on a peer".to_string(),
        Standing::StaleRun => "its run's fleet instance went silent".to_string(),
    }
}

/// Display the scheduler's decision, never a prediction from array position.
fn exclusive_says(request: &afkconvoy_core::exclusive::Observed) -> String {
    use afkconvoy_core::exclusive::State;
    let ticket = &request.ticket;
    let kind = request.kind.word();
    let mut doing = match request.scheduling.state {
        State::Running => {
            format!("{kind} · running, for {}", lasted(request.elapsed_seconds.unwrap_or(0)))
        }
        State::Eligible => format!(
            "{kind} · eligible now, waiting for grant, for {}",
            lasted(request.waiting_seconds)
        ),
        State::Waiting => {
            let holders: std::collections::BTreeSet<_> = request
                .scheduling
                .reasons
                .iter()
                .flat_map(|reason| reason.requests.iter().map(|holder| holder.number))
                .collect();
            let why: Vec<_> = request
                .scheduling
                .reasons
                .iter()
                .map(|reason| {
                    let requests: Vec<_> = reason
                        .requests
                        .iter()
                        .map(|holder| {
                            format!(
                                "request {} ({})",
                                holder.number,
                                if holder.state == State::Running {
                                    "running"
                                } else {
                                    "earlier reservation"
                                }
                            )
                        })
                        .collect();
                    format!(
                        "{}{}",
                        reason.explanation,
                        if requests.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", requests.join(", "))
                        }
                    )
                })
                .collect();
            format!(
                "{kind} · waits behind {} holding or reserving requests, for {} · {}",
                holders.len(),
                lasted(request.waiting_seconds),
                why.join("; ")
            )
        }
    };
    if let Some(validation) = &ticket.validation {
        doing.push_str(&format!(
            " · candidate {} · predecessor {} · {}",
            validation.id, validation.predecessor, validation.waiting
        ));
        if let Some(cause) = &validation.cause {
            doing.push_str(&format!(
                " · {cause} by {}",
                validation.cancelled_by.as_deref().unwrap_or("unknown")
            ));
        }
    }
    let dir = ticket.dir.rsplit('/').next().unwrap_or_default();
    match ticket.issue {
        Some(issue) => format!("{doing} · #{issue} in {dir}"),
        None => format!("{doing} · in {dir}"),
    }
}

/// The time of day of the moment `at`, in UTC: `14:05`.
fn clock(at: u64) -> String {
    format!("{:02}:{:02}", at % 86_400 / 3600, at % 3600 / 60)
}

fn gauge(out: &mut String, name: &str, value: usize, of: Option<usize>) {
    let of = of.map(|of| format!(" <small>of {of}</small>")).unwrap_or_default();
    let _ = write!(out, "<div class=\"gauge\"><dt>{name}</dt><dd>{value}{of}</dd></div>");
}

/// The page: a whole HTML document, the same text for the same page. It is
/// complete as it is; the script it names only keeps an open page fresh. Its
/// three parts are each one rendering, whoever made the page: the host
/// (`host_part`), what waits for a human across every repo (`human_part`),
/// and each repo (`repos_part`).
pub fn render(page: &Page) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{} — afkconvoy</title><style>{STYLE}</style></head>\n<body>\n<p class=\"lost\" \
         hidden>This page lost its host runner: it stopped, or was started again. What it shows \
         is the last tick it heard of.</p>\n<main id=\"panel\">\n",
        esc(&page.host.name),
    );
    host_part(&mut out, page);
    human_part(&mut out, page);
    repos_part(&mut out, page);
    out.push_str("</main>\n<script src=\"live.js\" defer></script>\n</body></html>\n");
    out
}

/// The host, once: who made the page, admission's reading, the hot workers
/// of every repo, the watch running and what the last host watch
/// found, the exclusive queue and the host kit.
fn host_part(out: &mut String, page: &Page) {
    let host = &page.host;
    let made = match page.live {
        true => "served by its host runner, made anew after every tick of any fleet instance on it"
            .to_string(),
        false => "one rebuild of each checkout, read just now: its host runner serves this page \
                  live, and <code>afkconvoy panel --open</code> says where"
            .to_string(),
    };
    let _ = writeln!(
        out,
        "<header class=\"top host\"><div class=\"who\"><h1>{}</h1><p>afkconvoy {} · {} · {made}\
         </p></div></header>",
        esc(&host.name),
        crate::VERSION,
        count(page.checkouts.len(), "checkout"),
    );
    out.push_str("<section class=\"host\">\n");
    admission(out, host);

    let hot: Vec<(String, &Hot)> = page
        .checkouts
        .iter()
        .flat_map(|checkout| {
            let hot = checkout.shown().map(|panel| panel.hot.as_slice()).unwrap_or_default();
            hot.iter().map(move |hot| (checkout.name(), hot))
        })
        .collect();
    if !hot.is_empty() {
        let hot: Vec<String> = hot
            .iter()
            .map(|(repo, hot)| {
                format!(
                    "<li data-hot=\"{worker}\"><b>{worker}</b> in {}, hot for {} — {}; holding at \
                     most {}</li>",
                    esc(repo),
                    lasted(host.now.saturating_sub(hot.since)),
                    esc(&hot.why),
                    esc(&hot.held),
                    worker = esc(&hot.worker),
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "<section class=\"hot\"><b>Hot workers</b>: their processes take more than their \
             share of the host.<ul>{}</ul></section>",
            hot.concat(),
        );
    }

    if let Some(session) = host.watch["session"].as_object() {
        let _ = write!(
            out,
            "<section class=\"hot\" data-watch=\"session\"><b>Watch session</b>: {} · {} · Terminal {}</section>",
            esc(session["id"].as_str().unwrap_or("")),
            esc(session["status"].as_str().unwrap_or("unavailable")),
            esc(session["terminal"].as_str().unwrap_or(""))
        );
    }
    if let Some(pending) = host.watch["pending"].as_array().filter(|p| !p.is_empty()) {
        out.push_str(
            "<section class=\"hot\" data-watch=\"pending\"><b>Pending watch episodes</b><ul>",
        );
        for item in pending {
            let _ = write!(
                out,
                "<li>{}: {}</li>",
                esc(item["episode"].as_str().unwrap_or("")),
                esc(item["item"]["reason"].as_str().unwrap_or(""))
            );
        }
        out.push_str("</ul></section>");
    }
    let asking = &host.asking;
    let waiting = asking["pending"].as_array().map_or(0, Vec::len);
    if asking["session"].is_object() || asking["current"].is_object() || waiting > 0 {
        let _ = write!(
            out,
            "<section class=\"hot\" data-asking=\"session\"><b>Asking session</b>: {} · Terminal {} · {} waiting",
            esc(asking["status"].as_str().unwrap_or("uncertain")),
            esc(asking["session"]["terminal"].as_str().unwrap_or("none")),
            waiting,
        );
        if let Some(question) = asking["current"]["question"]["human"]["question"].as_str() {
            let _ = write!(out, "<br>Current: {}", esc(question));
        }
        out.push_str("</section>");
    }
    for (key, title) in [("running", "Watch round"), ("last", "Last watch round")] {
        let watch = &host.watch[key];
        if watch.is_null() {
            continue;
        }
        let _ = write!(
            out,
            "<section class=\"hot\" data-watch=\"{}\"><b>{title}</b>: {} on {}<ul>",
            esc(key),
            esc(watch["id"].as_str().unwrap_or("")),
            esc(watch["harness"].as_str().unwrap_or(""))
        );
        for item in watch["items"].as_array().into_iter().flatten() {
            let _ = write!(out, "<li>{}</li>", esc(item["reason"].as_str().unwrap_or("")));
        }
        for did in watch["did"].as_array().into_iter().flatten() {
            let _ = write!(out, "<li>Did: {}</li>", esc(did.as_str().unwrap_or("")));
        }
        if !watch["report"].is_null() {
            let _ = write!(
                out,
                "<li>Report: <pre>{}</pre></li>",
                esc(&serde_json::to_string_pretty(&watch["report"]).unwrap_or_default())
            );
        }
        out.push_str("</ul></section>");
    }
    if let Some(why) = host.watch["unavailable"].as_str() {
        let _ =
            write!(out, "<section class=\"hot\"><b>Watch unavailable</b>: {}</section>", esc(why));
    }

    exclusive(out, host);

    kit(out, host);
    out.push_str("</section>\n");
}

/// Admission's reading of the host: what it holds back for, or that it
/// starts new workers, with what it read.
fn admission(out: &mut String, host: &Host) {
    let admission = match &host.admission {
        Ok(admission) => admission,
        Err(error) => {
            let _ = writeln!(
                out,
                "<p class=\"admission\"><b>Admission</b>: the host cannot be read: {}</p>",
                esc(error)
            );
            return;
        }
    };
    let read = esc(&admission.read_said().join(" · "));
    let holds: Vec<String> = admission.holds().iter().map(|hold| admission.said(hold)).collect();
    let _ = match holds.as_slice() {
        [] => writeln!(
            out,
            "<p class=\"admission\" data-admits=\"true\"><b>Admission</b> starts new workers: \
             {read}</p>"
        ),
        holds => writeln!(
            out,
            "<p class=\"holds\" data-admits=\"false\"><b>Admission holds back new workers</b>: {}. \
             It read {read}</p>",
            esc(&holds.join("; ")),
        ),
    };
}

/// The command lanes: the observation `afkconvoy queue` answers, each of its
/// lanes with the requests it lists there, running before waiting.
fn exclusive(out: &mut String, host: &Host) {
    out.push_str("<section class=\"exclusive\"><h2>Command lanes</h2>");
    let said = match &host.exclusive {
        Ok(said) => said,
        Err(error) => {
            let _ = writeln!(
                out,
                "<p>The exclusive queue cannot be read: {}</p></section>",
                esc(error)
            );
            return;
        }
    };
    let observation = crate::queue::snapshot_of(said, host.lanes, host.now);
    let _ = write!(
        out,
        "<p>{} worker lanes · {} used · {} free</p><p>{} fast gate lanes · {} used · {} free</p>",
        observation.worker.capacity,
        observation.worker.used,
        observation.worker.free,
        observation.fast.capacity,
        observation.fast.used,
        observation.fast.free
    );
    match said {
        None => out.push_str("<p>No host runner holds the queue.</p>"),
        Some((_, None)) => {
            out.push_str("<p>The host runner does not say whether it paused new grants.</p>")
        }
        Some((_, Some(true))) => out.push_str("<p>The host runner has paused new grants.</p>"),
        Some((_, Some(false))) => {}
    }
    // The worker lane, as `lane` names a ticket of no repo lane, and the
    // fast lane, each shown where a request is in it.
    let lanes = [("worker".to_string(), None), ("fast".to_string(), None)].into_iter().chain(
        observation.repos.iter().map(|repo| (format!("repo {}", repo.repo), Some(&repo.occupancy))),
    );
    for (lane, occupancy) in lanes {
        let mut requests = observation
            .running
            .iter()
            .chain(&observation.waiting)
            .filter(|request| request.lane == lane)
            .peekable();
        if occupancy.is_none() && requests.peek().is_none() {
            continue;
        }
        let _ = write!(out, "<h3>{} lane</h3>", esc(&lane));
        if let Some(occupancy) = occupancy {
            let _ = write!(out, "<p>{} used · {} free</p>", occupancy.used, occupancy.free);
        }
        out.push_str("<ul class=\"cards\">");
        for request in requests {
            let class = if request.ticket.ran.is_some() { "busy" } else { "queued" };
            let _ = write!(
                out,
                "<li class=\"card {class}\"><span class=\"title\"><code>{}</code></span>\
                 <span class=\"meta\"><span class=\"note\">{}</span></span></li>",
                esc(&request.ticket.command),
                esc(&exclusive_says(request)),
            );
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>\n");
}

/// What worker health holds of the host kit: each candidate a fault leaves
/// out, each failed observation and each worker in recovery.
fn kit_health(out: &mut String, health: &crate::health::Health) {
    if !health.faults.is_empty() {
        out.push_str("<section data-health-faults><h2>Unavailable candidates</h2><ul>");
        for fault in &health.faults {
            let _ = write!(
                out,
                "<li>{}: scope {}; model {}; next probe {}; evidence {}</li>",
                esc(fault.harness.key()),
                esc(fault.scope.key()),
                esc(fault.model.as_deref().unwrap_or("all")),
                fault.next_check,
                esc(&serde_json::json!(fault.evidence).to_string())
            );
        }
        out.push_str("</ul></section>");
    }
    if !health.observations_failed.is_empty() {
        let _ = write!(
            out,
            "<section data-health-observation-errors><h2>Waiting for observations</h2><pre>{}</pre></section>",
            esc(&serde_json::to_string_pretty(&health.observations_failed).unwrap_or_default())
        );
    }
    if !health.workers.is_empty() {
        out.push_str("<section data-worker-health><h2>Worker recovery</h2><table><tr><th>Worker</th><th>Orca state / outcome / agent type / since (epoch ms)</th><th>Action / successor</th><th>Evidence source</th><th>Next check</th><th>Evidence / failure</th></tr>");
        for worker in health.workers.iter().map(|worker| &worker.observed) {
            let _ = write!(
                out,
                "<tr><td>{}</td><td>{} / {} / {} / {}{}</td><td>{} {}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&worker.path.to_string_lossy()),
                esc(worker.orca["state"].as_str().unwrap_or("unknown")),
                esc(worker.orca["outcome"].as_str().unwrap_or("none")),
                esc(worker.orca["agentType"].as_str().unwrap_or("unknown")),
                worker.orca["stateStartedAt"],
                if worker.harness_mismatch {
                    format!(
                        " (recorded {}; Orca's harness is used)",
                        esc(worker.recorded_harness.as_deref().unwrap_or("unknown"))
                    )
                } else {
                    String::new()
                },
                esc(&worker.action),
                esc(worker.successor.as_deref().unwrap_or("")),
                esc(&worker.source),
                worker.next_check,
                esc(&match &worker.operator_reason {
                    Some(reason) => format!("waiting for the operator: {reason}"),
                    None => format!(
                        "{} {}",
                        serde_json::json!(worker.diagnosis),
                        serde_json::json!(worker.error)
                    ),
                })
            );
        }
        out.push_str("</table></section>");
    }
}

/// The host kit: every harness in it, in order, with its models in order,
/// each harness paused, since when and why.
fn kit(out: &mut String, host: &Host) {
    if let Ok(health) = &host.worker_health {
        kit_health(out, health);
    }
    for pause in &host.paused {
        let _ = writeln!(
            out,
            "<p class=\"lock\" data-paused=\"{key}\"><b>The harness {key} is paused on this \
             host</b>, for {}: {}. No worker is started on it; it is probed every hour.</p>",
            lasted(host.now.saturating_sub(pause.since)),
            esc(&pause.why),
            key = esc(pause.harness.key()),
        );
    }
    let kit = match &host.kit {
        Ok(kit) => kit,
        Err(error) => {
            let _ = writeln!(
                out,
                "<p class=\"holds\" data-kit><b>The host kit cannot be read</b>: {}</p>",
                esc(error)
            );
            return;
        }
    };
    let harnesses: Vec<String> = kit
        .harnesses
        .iter()
        .map(|held| {
            let models: Vec<String> = held.models.iter().map(|seen| esc(&seen.model)).collect();
            let models = match models.as_slice() {
                [] => "any model it picks".to_string(),
                models => models.join(", "),
            };
            let paused = host.paused.iter().any(|pause| pause.harness == held.harness);
            let paused = if paused { " <span class=\"note\">paused</span>" } else { "" };
            format!(
                "<li data-harness=\"{key}\"><b>{key}</b> {models}{paused}</li>",
                key = esc(held.harness.key())
            )
        })
        .collect();
    let _ = writeln!(
        out,
        "<section class=\"kit\"><h2>Host kit</h2><p class=\"by\">The harnesses and models this \
         host can start, in order of preference.</p>\
         <ul class=\"harnesses\">{}</ul></section>",
        harnesses.concat(),
    );
}

/// What waits for a human, across every repo: each fleet instance that is
/// broken, with why, and each issue, labelled with its repo.
fn human_part(out: &mut String, page: &Page) {
    out.push_str("<section class=\"human\"><h2>With a human</h2>");
    let now = page.host.now;
    let broken: Vec<(&Checkout, &Broken)> = page
        .checkouts
        .iter()
        .filter_map(|checkout| checkout.broken.as_ref().map(|broken| (checkout, broken)))
        .collect();
    if !broken.is_empty() {
        out.push_str("<ul class=\"cards broken\">");
        for (checkout, broken) in &broken {
            let next = match broken.next {
                Some(next) if next > now => {
                    format!("tried again in {}", lasted(next.saturating_sub(now)))
                }
                Some(_) => "tried again now".to_string(),
                None => "no host runner runs to try it again".to_string(),
            };
            let _ = write!(
                out,
                "<li class=\"card gone\" data-broken=\"{}\"><span class=\"repo\">{}</span> \
                 <span class=\"title\">its fleet instance is broken</span><span class=\"meta\">\
                 <span class=\"note\">{} · since {} ago · {next} · <code>afkconvoy run {}</code> \
                 starts it again now</span></span></li>",
                esc(&checkout.dir.display().to_string()),
                esc(&checkout.name()),
                esc(&broken.why),
                lasted(now.saturating_sub(broken.since)),
                esc(&checkout.dir.display().to_string()),
            );
        }
        out.push_str("</ul>");
    }
    let asked: Vec<(String, &Card)> = page
        .checkouts
        .iter()
        .flat_map(|checkout| {
            let asked = checkout.shown().map(|panel| panel.to_human.as_slice()).unwrap_or_default();
            asked.iter().map(move |card| (checkout.name(), card))
        })
        .collect();
    if !asked.is_empty() {
        out.push_str("<ul class=\"cards asked\">");
        for (repo, asked) in &asked {
            card(out, asked, Some(repo));
        }
        out.push_str("</ul>");
    }
    if broken.is_empty() && asked.is_empty() {
        out.push_str("<p class=\"none\">Nothing waits for a human.</p>");
    }
    out.push_str("</section>\n");
}

/// Each repo: a row naming every checkout with whether its fleet instance
/// is idle, the claims it holds and its frontier, each a link to its part;
/// then each part.
fn repos_part(out: &mut String, page: &Page) {
    out.push_str("<nav class=\"checkouts\"><ul>");
    for (at, checkout) in page.checkouts.iter().enumerate() {
        let standing = match (&checkout.part, &checkout.broken) {
            (_, Some(_)) => "<b class=\"broken\">broken</b>".to_string(),
            (Part::Starting, None) => "starting".to_string(),
            (Part::Unread(_), None) => "could not be read".to_string(),
            (Part::Shown(panel), None) => format!(
                "<b>{}</b> · holding {}/{} · frontier {}",
                if panel.idle { "idle" } else { "at work" },
                panel.held,
                panel.bound,
                panel.frontier,
            ),
        };
        let _ = write!(
            out,
            "<li><a href=\"#checkout-{}\">{}</a> <span>{standing}</span></li>",
            at + 1,
            esc(&checkout.name()),
        );
    }
    if page.checkouts.is_empty() {
        out.push_str(
            "<li>No checkout is on this host's checkout list: <code>afkconvoy run</code> in one \
             adds it.</li>",
        );
    }
    out.push_str("</ul></nav>\n");
    for (at, checkout) in page.checkouts.iter().enumerate() {
        let _ = write!(
            out,
            "<section class=\"repo\" id=\"checkout-{}\" data-checkout=\"{}\">",
            at + 1,
            esc(&checkout.dir.display().to_string()),
        );
        match &checkout.part {
            Part::Shown(panel) => repo(out, at + 1, panel),
            Part::Starting => {
                let _ = write!(
                    out,
                    "<h2 class=\"name\">{}</h2><p class=\"tick\">Its fleet instance's first tick \
                     is running. This part fills in when it ends.</p>",
                    esc(&checkout.name()),
                );
            }
            Part::Unread(why) => {
                let _ = write!(
                    out,
                    "<h2 class=\"name\">{}</h2><p class=\"lock\">It could not be read: {}</p>",
                    esc(&checkout.name()),
                    esc(why),
                );
            }
        }
        out.push_str("</section>\n");
    }
}

/// One repo's part, the `at`-th on the page: its fleet instance and its
/// gauges, its tick, what holds it back, its workflows as lines of their
/// stages with the issues at each, its landing train, its patrols, and what
/// its fleet leaves alone.
fn repo(out: &mut String, at: usize, panel: &Panel) {
    let who = match &panel.who {
        Some((instance, host)) => {
            format!("Fleet instance <b>{}</b> on {}", esc(instance), esc(host))
        }
        None => "Seen as no fleet instance".to_string(),
    };
    let issues = match &panel.tracker {
        Some(tracker) => format!(", its issues in <b>{}</b>", esc(tracker)),
        None => String::new(),
    };
    let _ = write!(
        out,
        "<header class=\"top\"><div class=\"who\"><h2 class=\"name\">{repo}</h2>\
         <p>{who}, landing on <b>{}</b>{issues}</p></div>\n<dl class=\"gauges\">",
        esc(&panel.base_branch),
        repo = esc(&panel.repo),
    );
    gauge(out, "Claims held", panel.held, Some(panel.bound));
    if panel.declared > 0 {
        gauge(out, "Taking no slot", panel.declared, None);
    }
    gauge(out, "Queued", panel.queued(), None);
    gauge(out, "With a human", panel.with_a_human(), None);
    out.push_str("</dl></header>\n");
    if !panel.candidates.is_empty() {
        out.push_str("<section class=\"candidates\"><h3>Candidate validation</h3><ul>");
        for candidate in &panel.candidates {
            let text = |key: &str| candidate[key].as_str().unwrap_or("");
            let _ = write!(
                out,
                "<li>PR {} · target <code>{}</code> · <code>{}</code> · predecessor <code>{}</code> · {} · {} {}",
                candidate["pr"],
                esc(text("target_branch")),
                esc(text("id")),
                esc(text("predecessor")),
                esc(text("waiting")),
                esc(text("cause")),
                esc(text("cancelled_by"))
            );
            if let Some(tip) = candidate["round"].as_str() {
                let _ = write!(out, " · in the round slow-gating at <code>{}</code>", esc(tip));
            }
            out.push_str("</li>");
        }
        out.push_str("</ul></section>");
    }
    if !panel.deliveries.is_empty() {
        out.push_str("<section class=\"deliveries\"><h3>Jira deliveries</h3><ul>");
        for line in &panel.deliveries {
            let _ = write!(out, "<li>{}</li>", esc(line));
        }
        out.push_str("</ul></section>");
    }

    match &panel.tick {
        Some(tick) => {
            // With no sleep after it, the next tick started as this page was made.
            let next = match tick.sleep {
                0 => "a tick is running".to_string(),
                sleep => format!("next tick in {}", lasted(sleep)),
            };
            let _ = writeln!(
                out,
                "<p class=\"tick\"><b>Tick {}</b> <span class=\"did\">{}</span> \
                 <time class=\"next\" data-next=\"{}\">{next}</time></p>",
                tick.count,
                esc(&tick.line),
                tick.ended.saturating_add(tick.sleep),
            );
        }
        None => out.push_str("<p class=\"tick\">One rebuild, read just now.</p>\n"),
    }
    if !panel.locks.is_empty() {
        let locks: Vec<String> = panel.locks.iter().map(|lock| format!("#{lock}")).collect();
        let _ = writeln!(
            out,
            "<p class=\"lock\"><b>A fleet lock stands: {}.</b> Only lock issues are worked \
             until it is closed.</p>",
            locks.join(", "),
        );
    }

    if !panel.casting.is_empty() {
        out.push_str("<details data-model-casting><summary>Model selection</summary><ul>");
        for issue in &panel.casting {
            let _ = write!(out, "<li><b>#{}</b>: {}", issue.number, esc(&issue.why));
            if let Some(ran) = &issue.running {
                let _ = write!(
                    out,
                    "; running {} on {}",
                    esc(ran.harness.key()),
                    esc(ran.model.as_deref().unwrap_or("the user's default"))
                );
            }
            out.push_str("<ul>");
            for stage in &issue.stages {
                let _ = write!(
                    out,
                    "<li>{}: {} on {} — {}",
                    esc(&stage.stage),
                    esc(stage.harness.map(|h| h.key()).unwrap_or("no eligible harness")),
                    esc(stage.model.as_deref().unwrap_or("the user's default")),
                    esc(&stage.why)
                );
                for skipped in &stage.skipped {
                    let _ = write!(out, "; skipped {}", esc(skipped));
                }
                out.push_str("</li>");
            }
            out.push_str("</ul></li>");
        }
        out.push_str("</ul></details>");
    }

    for (role, why) in &panel.nothing_safe {
        let _ = writeln!(
            out,
            "<p class=\"lock\" data-nothing-safe=\"{}\"><b>{}</b></p>",
            esc(role),
            esc(why),
        );
    }

    if !panel.codex_workers.is_empty() {
        out.push_str("<section class=\"codex-workers\"><h2>Codex workers</h2><ul>");
        for worker in &panel.codex_workers {
            let label = worker["issue"]
                .as_u64()
                .map(|n| format!("#{n}"))
                .unwrap_or_else(|| worker["worktree"].as_str().unwrap_or("worker").to_string());
            let text = |key: &str| esc(worker[key].as_str().unwrap_or(""));
            let model = worker["model"].as_str().unwrap_or("Codex default");
            let effort = worker["effort"].as_str().unwrap_or("Codex default");
            let _ = write!(
                out,
                "<li data-codex-worker=\"{}\"><b>{}</b>: model {} ({}); effort {} ({})</li>",
                esc(&label),
                esc(&label),
                esc(model),
                text("model_source"),
                esc(effort),
                text("effort_source")
            );
        }
        out.push_str("</ul></section>");
    }

    for lane in &panel.lanes {
        let _ = write!(
            out,
            "<section class=\"lane\" data-lane=\"{name}\"><h2>{name}</h2>\n<ol class=\"line\">",
            name = esc(&lane.name),
        );
        for one in &lane.nodes {
            node(out, one);
        }
        out.push_str("</ol></section>\n");
    }
    if panel.lanes.is_empty() && panel.tick.is_some() {
        out.push_str(
            "<p class=\"none wide\">No open issue follows a workflow. Put a flow label on one \
             to hand it to the fleet.</p>\n",
        );
    }

    if !panel.patrols.is_empty() {
        out.push_str("<section class=\"patrols\"><h2>Patrols</h2><ul class=\"cards\">");
        for row in &panel.patrols {
            let held =
                row.until.map(|until| format!(" data-until=\"{until}\"")).unwrap_or_default();
            let _ = write!(
                out,
                "<li class=\"card\" data-patrol=\"{name}\"{held}><span class=\"title\">{name}</span>\
                 <span class=\"meta\"><span class=\"note\">{}</span></span></li>",
                esc(&patrol_says(row, panel.now)),
                name = esc(&row.name),
            );
        }
        out.push_str("</ul></section>\n");
    }
    if let Some(cut) = &panel.cut {
        let _ = writeln!(
            out,
            "<section class=\"patrols\" data-cut><h2>Cut</h2><ul class=\"cards\"><li \
             class=\"card\"><span class=\"meta\"><span class=\"note\">{}</span></span></li>\
             </ul></section>",
            esc(cut)
        );
    }
    if !panel.off.is_empty() {
        let _ = write!(
            out,
            "<details class=\"off\" id=\"off-{at}\"><summary>{} the fleet leaves alone</summary>",
            count(panel.off.len(), "open issue"),
        );
        cards(out, "left", None, &panel.off);
        out.push_str("</details>\n");
    }
}

#[cfg(test)]
mod tests {
    use afkconvoy_core::admission::{Disk, Reading, SHARES};

    use super::*;

    fn part_of(repo: &str, now: u64) -> Panel {
        Panel {
            repo: repo.into(),
            base_branch: "main".into(),
            tracker: None,
            who: None,
            tick: None,
            held: 0,
            bound: 2,
            declared: 0,
            frontier: 0,
            idle: true,
            locks: vec![],
            lanes: vec![],
            to_human: vec![],
            off: vec![],
            patrols: vec![],
            cut: None,
            now,
            hot: vec![],
            casting: vec![],
            nothing_safe: vec![],
            codex_workers: vec![],
            candidates: vec![],
            deliveries: vec![],
        }
    }

    fn host_of(now: u64) -> Host {
        let reading = Reading {
            cores: 8,
            load: Some(150),
            memory_free: Some(40),
            disk: Some(Disk { free: 5 << 30, size: 100 << 30 }),
        };
        Host {
            name: "h1".into(),
            now,
            admission: Ok(Admission { shares: SHARES, said: vec![], reading, span: 600 }),
            exclusive: Ok(Some((vec![], Some(false)))),
            lanes: afkconvoy_core::exclusive::Lanes { worker: 1, fast: 1, slow: 1 },
            kit: Ok(Kit::default()),
            worker_health: Ok(crate::health::Health::default()),
            watch: serde_json::json!({}),
            asking: serde_json::json!({}),
            paused: vec![],
        }
    }

    fn checkout(dir: &str, part: Panel) -> Checkout {
        Checkout { dir: dir.into(), part: Part::Shown(Box::new(part)), broken: None }
    }

    /// The page of `parts`, each its own checkout, served live.
    fn page_of(host: Host, parts: Vec<Panel>) -> Page {
        let checkouts =
            parts.into_iter().map(|part| checkout(&format!("/w/{}", part.repo), part)).collect();
        Page { host, live: true, checkouts }
    }

    #[test]
    fn the_page_is_the_host_once_then_what_waits_for_a_human_then_each_repo() {
        let mut acme = part_of("acme", 1000);
        acme.to_human = vec![Card::new(4, "Pick a name").noted("escalated")];
        acme.idle = false;
        (acme.held, acme.frontier) = (1, 3);
        let mut widgets = part_of("widgets", 1000);
        widgets.to_human = vec![Card::new(9, "Ship it?").noted("at a human's stage")];
        let mut page = page_of(host_of(1000), vec![acme, widgets]);
        page.checkouts.push(Checkout {
            dir: "/w/gizmo".into(),
            part: Part::Starting,
            broken: Some(Broken {
                why: "it panicked: <boom>".into(),
                since: 940,
                next: Some(1060),
            }),
        });
        let html = render(&page);
        let at = |text: &str| html.find(text).unwrap_or_else(|| panic!("no {text}: {html}"));
        assert!(at("<header class=\"top host\">") < at("<section class=\"human\">"));
        assert!(at("<section class=\"human\">") < at("<nav class=\"checkouts\">"));
        assert!(at("<nav class=\"checkouts\">") < at("<section class=\"repo\" id=\"checkout-1\""));
        assert_eq!(html.matches("<h1>").count(), 1, "the host is shown once: {html}");
        assert!(html.contains("<h1>h1</h1>"), "{html}");
        // admission's reading, in words
        assert!(html.contains("data-admits=\"true\"><b>Admission</b> starts new workers: a load of 1.50 on 8 cores · 40% of its memory free · 5.0 GiB of the checkout's disk free"), "{html}");
        // with a human: the broken fleet instance, with why, then each issue
        // labelled with its repo
        assert!(at("data-broken=\"/w/gizmo\"") < at("<ul class=\"cards asked\">"));
        assert!(
            html.contains("it panicked: &lt;boom&gt; · since 1m ago · tried again in 1m"),
            "{html}"
        );
        assert!(html.contains("<span class=\"repo\">acme</span> <span class=\"num\">#4</span>"));
        assert!(html.contains("<span class=\"repo\">widgets</span> <span class=\"num\">#9</span>"));
        // the row naming every checkout, each a link to its part
        assert!(html.contains("<li><a href=\"#checkout-1\">acme</a> <span><b>at work</b> · holding 1/2 · frontier 3</span></li>"), "{html}");
        assert!(html.contains("<a href=\"#checkout-2\">widgets</a> <span><b>idle</b> · holding 0/2 · frontier 0</span>"), "{html}");
        assert!(
            html.contains(
                "<a href=\"#checkout-3\">gizmo</a> <span><b class=\"broken\">broken</b></span>"
            ),
            "{html}"
        );
        assert!(html.contains("id=\"checkout-3\" data-checkout=\"/w/gizmo\"><h2 class=\"name\">gizmo</h2><p class=\"tick\">Its fleet instance's first tick is running."), "{html}");
        // the page made by one rebuild of each says so, and where the live one is
        assert!(html.contains("made anew after every tick of any fleet instance on it"));
        page.live = false;
        assert!(render(&page).contains("<code>afkconvoy panel --open</code> says where"));
    }

    #[test]
    fn a_candidate_says_the_round_whose_slow_gate_it_waits_on() {
        let mut acme = part_of("acme", 1000);
        let row = |id: &str, waiting: &str, round: Option<&str>| {
            serde_json::json!({"pr": 7, "target_branch": "main", "id": id, "predecessor": "m0",
                "waiting": waiting, "cause": null, "cancelled_by": null, "round": round})
        };
        acme.candidates = vec![
            row("c1", "waiting_for_round", Some("c2")),
            row("c2", "slow_gating", Some("c2")),
            row("c3", "waiting_for_round", None),
        ];
        let html = render(&page_of(host_of(1000), vec![acme]));
        assert!(
            html.contains("<code>c1</code> · predecessor <code>m0</code> · waiting_for_round ·   · in the round slow-gating at <code>c2</code></li>"),
            "{html}"
        );
        assert!(
            html.contains(
                "<code>c3</code> · predecessor <code>m0</code> · waiting_for_round ·  </li>"
            ),
            "{html}"
        );
    }

    #[test]
    fn nothing_waiting_for_a_human_is_said_so_and_no_checkout_on_the_list_too() {
        let html = render(&page_of(host_of(1000), vec![part_of("acme", 1000)]));
        assert!(html.contains("<p class=\"none\">Nothing waits for a human.</p>"), "{html}");
        let html = render(&page_of(host_of(1000), vec![]));
        assert!(html.contains("No checkout is on this host's checkout list"), "{html}");
        let mut host = host_of(1000);
        host.admission = Err("sysctl: <none>".into());
        let html = render(&page_of(host, vec![]));
        assert!(!html.contains("Host sleep"), "{html}");
        assert!(html.contains("the host cannot be read: sysctl: &lt;none&gt;"), "{html}");
    }

    #[test]
    fn the_command_lanes_are_the_queue_observation_s_lanes() {
        let ticket = |number: u64, repo_lane: Option<&str>, ran: Option<u64>| {
            afkconvoy_core::exclusive::Ticket {
                number,
                pid: 4000 + number as u32,
                command: format!("cmd-{number}"),
                dir: format!("/w/{number}"),
                issue: None,
                cut: false,
                repo_lane: repo_lane.map(String::from),
                repo: repo_lane.map(String::from),
                is_turn: false,
                candidate: false,
                validation: None,
                gate: None,
                asked: 900 + number,
                ran,
            }
        };
        let said = Some((
            vec![
                ticket(1, None, Some(950)),
                ticket(2, None, None),
                ticket(3, Some("/a/.git"), Some(960)),
            ],
            None,
        ));
        let mut host = host_of(1000);
        host.exclusive = Ok(said.clone());
        let html = render(&page_of(host, vec![]));
        let observation = crate::queue::observation(
            said,
            afkconvoy_core::exclusive::Lanes { worker: 1, fast: 1, slow: 1 },
            1000,
        );
        let section = html.split("<h2>Command lanes</h2>").nth(1).unwrap();
        let section = section.split("</section>").next().unwrap();
        let worker = section.split("<h3>worker lane</h3>").nth(1).unwrap();
        let (worker, repo) = worker.split_once("<h3>repo /a/.git lane</h3>").unwrap();
        for request in observation["running"]
            .as_array()
            .unwrap()
            .iter()
            .chain(observation["waiting"].as_array().unwrap())
        {
            let card = format!("<code>{}</code>", request["command"].as_str().unwrap());
            let shown = if request["lane"] == "worker" { worker } else { repo };
            assert!(shown.contains(&card), "{} not in its lane: {section}", request["lane"]);
        }
        assert!(section.contains("1 worker lanes · 1 used · 0 free"), "{section}");
        assert!(repo.contains("1 used · 0 free"), "{section}");
        assert!(section.contains("does not say whether it paused"), "{section}");
        assert!(!section.contains("waiting_for_capacity"), "{section}");
        let mut host = host_of(1000);
        host.exclusive = Err("the host runner closed".into());
        let html = render(&page_of(host, vec![]));
        assert!(html.contains("The exclusive queue cannot be read: the host runner closed"));
        assert!(!html.contains("worker lanes"), "an unread queue is not an empty one: {html}");
    }

    #[test]
    fn a_paused_harness_is_shown_with_since_when_and_why() {
        let mut host = host_of(1000 + 7200);
        host.paused = vec![afkconvoy_core::casting::Paused {
            harness: afkconvoy_core::harness::Harness::Pi,
            since: 1000,
            why: "subscription <disabled>".into(),
            probed: None,
        }];
        let page = render(&page_of(host, vec![]));
        assert!(
            page.contains(
                "data-paused=\"pi\"><b>The harness pi is paused on this host</b>, for 2h"
            ),
            "{page}"
        );
        assert!(page.contains("subscription &lt;disabled&gt;"), "{page}");
        assert!(page.contains("<h2>Host kit</h2>"), "{page}");
    }

    #[test]
    fn a_role_that_can_run_nothing_safely_is_named() {
        let mut part = part_of("r", 1000);
        part.nothing_safe = vec![(
            "lead".into(),
            "lead has no harness on this host it holds limits for: <x>".into(),
        )];
        let page = render(&page_of(host_of(1000), vec![part]));
        assert!(
            page.contains(
                "data-nothing-safe=\"lead\"><b>lead has no harness on this host it holds limits \
                 for: &lt;x&gt;</b>"
            ),
            "{page}"
        );
    }

    #[test]
    fn a_title_is_never_read_as_markup() {
        let mut part = part_of("r", 1000);
        part.who = Some(("alpha".into(), "h1".into()));
        part.off = vec![Card::new(7, "<script>alert(\"x\")</script> & more")];
        let mut page = page_of(host_of(1000), vec![part]);
        let html = render(&page);
        assert!(html.contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; more"));
        assert_eq!(html.matches("<script").count(), 1, "only the page's own");
        // the same page is the same text
        assert_eq!(html, render(&page));
        assert!(html.contains("<p class=\"tick\">One rebuild, read just now.</p>"), "{html}");
        let Part::Shown(part) = &mut page.checkouts[0].part else { unreachable!() };
        part.tick = Some(Tick { count: 3, line: "nothing to do".into(), ended: 1000, sleep: 90 });
        let ticked = render(&page);
        assert!(
            ticked.contains("<b>Tick 3</b>")
                && ticked.contains("data-next=\"1090\">next tick in 1m<")
        );
        assert!(!ticked.contains("Hot workers"));
    }

    #[test]
    fn the_hot_workers_and_the_watch_of_every_repo_are_the_host_s() {
        let episode = afkconvoy_core::hot::Episode {
            since: 280,
            why: afkconvoy_core::hot::Why::Held,
            millicores: 6200,
            resident: 3 << 29,
            written: 0,
            held: 5 << 30,
        };
        let mut part = part_of("acme", 1000);
        part.hot = vec![Hot::from(&HotNow {
            worker: "#7".into(),
            episode,
            path: "/w/a".into(),
            branch: "a".into(),
            issue: Some(7),
            why: "it holds 5.0 GiB in its worktree, past <4.0 GiB>".into(),
        })];
        let page = page_of(host_of(1000), vec![part]);
        let hot = render(&page);
        assert!(hot.contains("<li data-hot=\"#7\"><b>#7</b> in acme, hot for 12m — it holds 5.0 GiB in its worktree, past &lt;4.0 GiB&gt;; holding at most 6.2 cores, 1.5 GiB resident, 0 B written in 10 minutes, 5.0 GiB held in its worktree</li>"), "{hot}");
        assert!(hot.find("Hot workers").unwrap() < hot.find("<nav").unwrap(), "{hot}");
        assert!(!hot.contains("Watch"), "{hot}");
    }

    #[test]
    fn a_length_of_time_is_said_in_its_largest_units() {
        assert_eq!([lasted(40), lasted(725), lasted(11_100)], ["40s", "12m", "3h 5m"]);
        assert_eq!([count(1, "PR"), count(0, "PR")], ["1 PR", "0 PRs"]);
    }

    #[test]
    fn the_host_shows_its_watch_and_what_the_last_one_did() {
        let mut host = host_of(1000);
        host.watch = serde_json::json!({
            "running": {"id": "watch-1", "harness": "codex",
                "items": [{"reason": "hot worker <7>"}]},
            "last": {"id": "watch-0", "harness": "claude-code",
                "did": ["ended pid <4242>"]}
        });
        let page = render(&page_of(host, vec![]));
        for expected in [
            "data-watch=\"running\"",
            "data-watch=\"last\"",
            "hot worker &lt;7&gt;",
            "Did: ended pid &lt;4242&gt;",
        ] {
            assert!(page.contains(expected), "{page}");
        }
        assert!(!page.to_lowercase().contains("inspection"));
    }
}

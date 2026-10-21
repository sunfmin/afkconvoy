//! The panel: the page that shows a human where the whole fleet stands — each
//! workflow as a line of stages, the issues at work, queued and held back at
//! each, and the landing train they end on. Like a status board it is rendered
//! from what a tick knows and never read back.
//!
//! `of` decides what goes where and `render` puts it into HTML; both are pure.
//! `read` is the one function here that asks anything: orca, after each
//! worker's state.

use std::fmt::Write;

use afkconvoy_core::exclusive::Ticket;
use afkconvoy_core::patrol::{self, Standing};
use afkconvoy_core::plan::{Excluded, Status, WorkingSet};
use afkconvoy_core::position::Position;
use afkconvoy_core::team::{LAND, Performer, Team, Work, Workflow};
use afkconvoy_core::tick::{Outcome, TickStep};
use afkconvoy_core::worker::WorkerState;

use crate::claims::Me;
use crate::exclusive::Queue;
use crate::fleet::Fleet;
use crate::hot::{HotNow, held_said};
use crate::orca::{Source, Worktree};
use crate::stage::Flows;

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
}

impl Card {
    fn new(number: u64, title: &str) -> Card {
        Card { number, title: title.to_string(), pr: None, peer: None, worker: None, note: None }
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

/// Everything the page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub repo: String,
    pub base_branch: String,
    /// The fleet instance whose view this is, and its host; `None` of nobody's:
    /// every claim is then another instance's.
    pub who: Option<(String, String)>,
    /// `None` of a panel no `run` made: it shows a rebuild, not a tick.
    pub tick: Option<Tick>,
    /// The claims held that take a slot, of the `bound` the fleet may hold.
    pub held: usize,
    pub bound: usize,
    /// The claims held whose worker declared a verdict not acted on yet: they
    /// take no slot, and are counted apart.
    pub declared: usize,
    pub locks: Vec<u64>,
    pub lanes: Vec<Lane>,
    /// The issues whose PR is on the landing train.
    pub train: Vec<Card>,
    /// The issues that wait for a human, each with why and the line that
    /// answers it: the working set's `waiting_on_human`, as `doctor` gives it.
    pub to_human: Vec<Card>,
    /// Every other open issue, with why the fleet leaves it.
    pub off: Vec<Card>,
    /// The patrols the repo names, each as it stands.
    pub patrols: Vec<patrol::Row>,
    /// The host's `exclusive` queue, whatever its repo: the first runs alone,
    /// and the rest wait their turn, next first.
    pub exclusive: Vec<Ticket>,
    /// When the page was made, in epoch seconds: what a patrol's wait is
    /// counted from.
    pub now: u64,
    /// The workers hot as the tick ended, as the `run` that made the page
    /// found them; none of a panel no `run` made.
    pub hot: Vec<HotNow>,
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
        Node::new(LAND, "the same worker joins the train".to_string()),
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
        Position::Landing => Some(LAND.to_string()),
        _ => position.stage().map(|stage| stage.name.clone()),
    };
    Some((workflow.name.clone(), stage))
}

/// What the tick's `step` says of the issue it was about, once it ended in
/// `outcome`; `None` for a step that shows on no card.
fn said(step: TickStep, outcome: Outcome) -> Option<(u64, &'static str)> {
    let issue = match step {
        TickStep::Judge { issue, .. }
        | TickStep::Watch { issue }
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
            who: me.map(|me| (me.instance.as_str().to_string(), me.host.clone())),
            tick: None,
            held: 0,
            bound: fleet.config.concurrency(),
            declared: 0,
            locks: vec![],
            lanes: vec![],
            train: vec![],
            to_human: vec![],
            off: vec![],
            patrols: vec![],
            exclusive: vec![],
            now: fleet.now,
            hot: vec![],
        }
    }

    /// Puts every open issue of `seen` where the page shows it: at work or
    /// queued at its stage, held back there, on the train, or left — and,
    /// whatever stage it is held at, with a human when it waits for one.
    pub fn show(mut self, seen: &Seen) -> Panel {
        let set = seen.set;
        self.lanes = lanes(seen);
        self.locks = set.locks.clone();
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
                Status::Joined => put.panel.train.push(card),
                Status::Joining => put.at(Some(LAND), card, List::AtWork),
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
            let card = Card::new(issue.number, &issue.title);
            put.at(issue.stage.as_deref(), card, List::Queued);
        }
        for waiting in &set.waiting_on_human {
            let note = format!("{} — {}", waiting.why, waiting.answer);
            put.panel.to_human.push(Card::new(waiting.issue, &waiting.title).noted(&note));
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
/// `flows`, and a tick — if one was made — carried out `did`. Asks orca after
/// the worker of every claim of mine at work; one orca cannot speak of is
/// shown with no state rather than failing the page.
pub fn read(
    fleet: &Fleet,
    me: Option<&Me>,
    set: &WorkingSet,
    flows: &Flows,
    did: &[(TickStep, Outcome)],
) -> Panel {
    let source = Source::of(&fleet.repo).ok();
    let state = |issue: u64| {
        Worktree::worker_of(source.as_ref()?, issue, fleet.now).ok().map(|(_, state)| state)
    };
    let at_work = |status| matches!(status, Status::NoPr | Status::AtStage | Status::Joining);
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
    panel.declared = set.declared();
    panel.held = set.mine.len() - panel.declared;
    panel.bound = set.bound(fleet.config.concurrency()) - panel.declared;
    // Soft: a queue that cannot be read shows nothing.
    panel.exclusive = Queue::of_host().and_then(|queue| queue.standing()).unwrap_or_default();
    panel
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

fn card(out: &mut String, card: &Card) {
    let (class, worker) = match card.worker {
        Some(WorkerState::Busy) => ("busy", Some("busy".to_string())),
        Some(WorkerState::Idle { quiet: Some(quiet) }) => {
            ("stopped", Some(format!("stopped {} ago", lasted(quiet))))
        }
        Some(WorkerState::Idle { quiet: None }) => ("stopped", Some("stopped".to_string())),
        Some(WorkerState::Gone) => ("gone", Some("no worker".to_string())),
        None => ("", None),
    };
    let _ = write!(
        out,
        "<li class=\"card {class}\" data-issue=\"{n}\"><span class=\"num\">#{n}</span> \
         <span class=\"title\">{}</span>",
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
        card(out, one);
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

/// What the `exclusive` command `ticket` is doing at `now` — the one that
/// runs, or one waiting in `place` — and whose it is, in words.
fn exclusive_says(ticket: &Ticket, place: usize, now: u64) -> String {
    let doing = match ticket.ran {
        Some(ran) => format!("runs alone, for {}", lasted(now.saturating_sub(ran))),
        None if place == 0 => "about to run".to_string(),
        None => {
            format!("waits behind {place}, for {}", lasted(now.saturating_sub(ticket.asked)))
        }
    };
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

/// The page: a whole HTML document, the same text for the same panel. It is
/// complete as it is; the script it names only keeps an open page fresh.
pub fn render(panel: &Panel) -> String {
    let mut out = String::new();
    let who = match &panel.who {
        Some((instance, host)) => {
            format!("Fleet instance <b>{}</b> on {}", esc(instance), esc(host))
        }
        None => "Seen as no fleet instance".to_string(),
    };
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{repo} — afkconvoy</title><style>{STYLE}</style></head>\n<body>\n<p class=\"lost\" hidden>This page lost its fleet instance: <code>afkconvoy run</code> \
         stopped, or was started again. What it shows is the last tick it heard of.</p>\n\
         <main id=\"panel\">\n<header class=\"top\"><div class=\"who\"><h1>{repo}</h1>\
         <p>{who}, landing on <b>{}</b></p></div>\n<dl class=\"gauges\">",
        esc(&panel.base_branch),
        repo = esc(&panel.repo),
    );
    gauge(&mut out, "Claims held", panel.held, Some(panel.bound));
    if panel.declared > 0 {
        gauge(&mut out, "Declared", panel.declared, None);
    }
    gauge(&mut out, "Queued", panel.queued(), None);
    gauge(&mut out, "On the train", panel.train.len(), None);
    gauge(&mut out, "With a human", panel.with_a_human(), None);
    out.push_str("</dl></header>\n");

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
        None if panel.lanes.is_empty() => out.push_str(
            "<p class=\"tick\">The first tick is running. This page fills in when it ends.</p>\n",
        ),
        None => out.push_str(
            "<p class=\"tick\">One rebuild, read just now. No fleet instance keeps this page \
             fresh: <code>afkconvoy run --panel</code> does, and so does one that is running \
             already, asked with <code>afkconvoy panel --open</code>.</p>\n",
        ),
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

    if !panel.hot.is_empty() {
        let hot: Vec<String> = panel
            .hot
            .iter()
            .map(|hot| {
                format!(
                    "<li data-hot=\"{worker}\"><b>{worker}</b> hot on {} for {}, holding at most \
                     {}</li>",
                    hot.episode.why.word(),
                    lasted(panel.now.saturating_sub(hot.episode.since)),
                    esc(&held_said(&hot.episode)),
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

    for lane in &panel.lanes {
        let _ = write!(
            out,
            "<section class=\"lane\" data-lane=\"{name}\"><h2>{name}</h2>\n<ol class=\"line\">",
            name = esc(&lane.name),
        );
        for one in &lane.nodes {
            node(&mut out, one);
        }
        out.push_str("</ol></section>\n");
    }
    if panel.lanes.is_empty() && panel.tick.is_some() {
        out.push_str(
            "<p class=\"none wide\">No open issue follows a workflow. Put a flow label on one \
             to hand it to the fleet.</p>\n",
        );
    }

    let _ = write!(
        out,
        "<section class=\"train\"><h2>Landing train</h2><p class=\"by\">{}, gated, then landed \
         on {} together</p>",
        count(panel.train.len(), "PR"),
        esc(&panel.base_branch),
    );
    let _ = write!(
        out,
        "<div class=\"rail\"><span class=\"base\">{}</span></div>",
        esc(&panel.base_branch)
    );
    cards(&mut out, "cars", None, &panel.train);
    if panel.train.is_empty() {
        out.push_str("<p class=\"none\">No PR is on the train.</p>");
    }
    out.push_str("</section>\n");

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
    if !panel.exclusive.is_empty() {
        out.push_str(
            "<section class=\"exclusive\"><h2>Run alone on this host</h2><ul class=\"cards\">",
        );
        for (place, ticket) in panel.exclusive.iter().enumerate() {
            let class = if place == 0 { "busy" } else { "queued" };
            let _ = write!(
                out,
                "<li class=\"card {class}\"><span class=\"title\"><code>{}</code></span>\
                 <span class=\"meta\"><span class=\"note\">{}</span></span></li>",
                esc(&ticket.command),
                esc(&exclusive_says(ticket, place, panel.now)),
            );
        }
        out.push_str("</ul></section>\n");
    }
    if !panel.to_human.is_empty() {
        out.push_str("<section class=\"human\"><h2>With a human</h2>");
        cards(&mut out, "asked", None, &panel.to_human);
        out.push_str("</section>\n");
    }
    if !panel.off.is_empty() {
        let _ = write!(
            out,
            "<details class=\"off\"><summary>{} the fleet leaves alone</summary>",
            count(panel.off.len(), "open issue"),
        );
        cards(&mut out, "left", None, &panel.off);
        out.push_str("</details>\n");
    }
    out.push_str("</main>\n<script src=\"live.js\" defer></script>\n</body></html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_s_exclusive_queue_shows_what_runs_alone_and_what_waits() {
        let ticket = |command: &str, issue, ran| Ticket {
            number: 1,
            pid: 1,
            command: command.into(),
            dir: "/w/acme".into(),
            issue,
            asked: 700,
            ran,
        };
        let running = ticket("cargo test <all>", Some(12), Some(820));
        let waiting = ticket("make bench", None, None);
        assert_eq!(exclusive_says(&running, 0, 1000), "runs alone, for 3m · #12 in acme");
        assert_eq!(exclusive_says(&waiting, 1, 1000), "waits behind 1, for 5m · in acme");
        assert_eq!(exclusive_says(&ticket("x", None, None), 0, 1000), "about to run · in acme");
        let panel = Panel { exclusive: vec![running, waiting], ..page_of(1000) };
        let page = render(&panel);
        assert!(page.contains("<h2>Run alone on this host</h2>"), "{page}");
        assert!(page.contains("<code>cargo test &lt;all&gt;</code>"), "{page}");
        assert!(page.find("cargo test").unwrap() < page.find("make bench").unwrap());
    }

    fn page_of(now: u64) -> Panel {
        Panel {
            repo: "r".into(),
            base_branch: "main".into(),
            who: None,
            tick: None,
            held: 0,
            bound: 2,
            declared: 0,
            locks: vec![],
            lanes: vec![],
            train: vec![],
            to_human: vec![],
            off: vec![],
            patrols: vec![],
            exclusive: vec![],
            now,
            hot: vec![],
        }
    }

    #[test]
    fn a_title_is_never_read_as_markup() {
        let mut panel = Panel {
            repo: "r".into(),
            base_branch: "main".into(),
            who: Some(("alpha".into(), "h1".into())),
            tick: None,
            held: 0,
            bound: 2,
            declared: 0,
            locks: vec![],
            lanes: vec![],
            train: vec![Card::new(7, "<script>alert(\"x\")</script> & more")],
            to_human: vec![],
            off: vec![],
            patrols: vec![],
            exclusive: vec![],
            now: 1000,
            hot: vec![],
        };
        let page = render(&panel);
        assert!(page.contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; more"));
        assert_eq!(page.matches("<script").count(), 1, "only the page's own");
        // the same panel is the same page
        assert_eq!(page, render(&panel));
        assert!(!page.contains("Run alone on this host"), "no queue, no section");
        panel.tick = Some(Tick { count: 3, line: "nothing to do".into(), ended: 1000, sleep: 90 });
        let ticked = render(&panel);
        assert!(
            ticked.contains("<b>Tick 3</b>")
                && ticked.contains("data-next=\"1090\">next tick in 1m<")
        );
        assert!(!ticked.contains("Hot workers"));
        let episode = afkconvoy_core::hot::Episode {
            since: 280,
            why: afkconvoy_core::hot::Why::Held,
            millicores: 6200,
            resident: 3 << 29,
            written: 0,
            held: 5 << 30,
        };
        panel.hot = vec![HotNow { worker: "#7".into(), episode }];
        let hot = render(&panel);
        assert!(hot.contains("<li data-hot=\"#7\"><b>#7</b> hot on disk held for 12m, holding at most 6.2 cores, 1.5 GiB resident, 0 B written in 10 minutes, 5.0 GiB held in its worktree</li>"), "{hot}");
    }

    #[test]
    fn a_length_of_time_is_said_in_its_largest_units() {
        assert_eq!([lasted(40), lasted(725), lasted(11_100)], ["40s", "12m", "3h 5m"]);
        assert_eq!([count(1, "PR"), count(0, "PR")], ["1 PR", "0 PRs"]);
    }
}

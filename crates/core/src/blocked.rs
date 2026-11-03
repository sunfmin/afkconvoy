//! A `blocked` verdict: where each issue it names stands, and what that comes
//! to for the claim.

use flux_rs::attrs::*;

use crate::config::Config;
use crate::plan::{LabelBar, label_bar};
use crate::record::fleet_number;
use crate::team::FlowLabels;

/// The word a `blocked` verdict names the next cut of its repo by, in place of
/// an issue's number or beside them (`blocked_by=cut`).
pub const CUT: &str = "cut";

/// What a `blocked` verdict waits on: the issues it names, and whether it
/// waits on the next cut of its own repo — a version that is not installed
/// yet (docs/adr/0056).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockedOn {
    pub issues: Vec<u64>,
    pub cut: bool,
}

impl BlockedOn {
    /// Waiting on these issues, and on no cut.
    pub fn issues(issues: Vec<u64>) -> BlockedOn {
        BlockedOn { issues, cut: false }
    }

    /// What a `blocked_by=` value names: numbers — a hand may write `#3` — and
    /// the word `cut`. Anything else in the list is dropped.
    pub fn read(raw: Option<&str>) -> BlockedOn {
        let mut on = BlockedOn::default();
        for item in raw.unwrap_or_default().split(',').map(str::trim) {
            if item == CUT {
                on.cut = true;
            } else if let Some(number) = fleet_number(item.strip_prefix('#').unwrap_or(item)) {
                on.issues.push(number);
            }
        }
        on
    }

    /// The value it is written as: the numbers, then `cut` when it waits on
    /// the cut; empty when it names nothing.
    pub fn word(&self) -> String {
        let mut words: Vec<String> = self.issues.iter().map(u64::to_string).collect();
        if self.cut {
            words.push(CUT.to_string());
        }
        words.join(",")
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty() && !self.cut
    }
}

/// Whether a `blocked` verdict waits on the next cut of its repo, and whether
/// one will come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutWait {
    /// It does not name the cut.
    No,
    /// It does, and a cut will come: one is running, failed and waits for an
    /// answer, or is due or will be once it is.
    Coming,
    /// It does, and no cut will come, for this reason.
    Never(String),
}

/// How many issues the chain of open blockers behind a verdict's blockers may
/// hold before the walk gives up: a dependency graph that deep is not read
/// issue by issue.
pub const WALK_LIMIT: usize = 200;

/// One issue a `blocked` verdict names, as GitHub has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerRead {
    pub closed: bool,
    /// Why it was closed, in GitHub's word.
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub pull_request: bool,
}

/// One named blocker with what the backlog holds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub number: u64,
    /// `None` when it could not be read — which is never "closed".
    pub read: Option<BlockerRead>,
    /// A claim exists for it, whoever holds it: a live one is being worked, a
    /// stale one is taken over.
    pub claimed: bool,
    /// An open PR closes it.
    pub open_pr: bool,
}

/// Where one named blocker stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Done: it blocks nothing any more.
    Closed,
    /// Open, and the backlog will resolve it with no human.
    Waiting,
    /// Nothing will resolve it, and why — worded to follow its number.
    Unmet(String),
}

/// The open blockers GitHub records for one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub issue: u64,
    pub open_blockers: Vec<u64>,
}

/// One named blocker's standing, before the cycle check. `flows` are the
/// flow labels of a bound repo's team.
fn alone(named: &Named, config: &Config, flows: &FlowLabels) -> Standing {
    let Some(read) = &named.read else {
        return Standing::Unmet("could not be read (it may not exist)".to_string());
    };
    if read.pull_request {
        return Standing::Unmet("is a pull request, not an issue".to_string());
    }
    if read.closed {
        // Closed without the work being done clears nothing.
        return match read.state_reason.as_deref() {
            Some("not_planned") => Standing::Unmet("was closed as not planned".to_string()),
            Some("duplicate") => Standing::Unmet("was closed as a duplicate".to_string()),
            _ => Standing::Closed,
        };
    }
    match label_bar(&read.labels, config, flows) {
        // A bar no claim overrides: nothing will ever dispatch it.
        Some(bar @ LabelBar::Epic(_)) => {
            Standing::Unmet(format!("will never be dispatched: {}", bar.reason()))
        }
        Some(bar @ (LabelBar::NotReady(_) | LabelBar::NoFlow | LabelBar::Repos(_))) => {
            if named.claimed || named.open_pr {
                Standing::Waiting
            } else {
                Standing::Unmet(format!(
                    "is open but no fleet will work it (unclaimed, no PR, {})",
                    bar.reason()
                ))
            }
        }
        None => Standing::Waiting,
    }
}

/// Whether issue `start` depends — directly or through any chain of open
/// blockers — on issue `target`.
pub fn depends_on(start: u64, target: u64, edges: &[Edge]) -> bool {
    let mut seen: Vec<u64> = Vec::new();
    let mut todo = vec![start];
    while let Some(issue) = todo.pop() {
        if issue == target {
            return true;
        }
        if seen.contains(&issue) {
            continue;
        }
        seen.push(issue);
        for edge in edges {
            if edge.issue == issue {
                todo.extend(edge.open_blockers.iter().copied());
            }
        }
    }
    false
}

/// Where `named` stands as a blocker of `issue`. `edges` covers everything
/// reachable from the open blockers named (`walk`): a blocker that itself
/// depends on `issue` is unmet — waiting on it would close a cycle neither
/// side ever leaves. `flows` are the flow labels of a bound repo's team.
pub fn standing(
    issue: u64,
    named: &Named,
    config: &Config,
    flows: &FlowLabels,
    edges: &[Edge],
) -> Standing {
    if named.number == issue {
        return Standing::Unmet("is the issue itself".to_string());
    }
    let alone = alone(named, config, flows);
    if alone == Standing::Waiting && depends_on(named.number, issue, edges) {
        return Standing::Unmet(format!(
            "already depends on #{issue}: waiting on it would close a cycle"
        ));
    }
    alone
}

/// The named blockers whose own blockers the cycle check must read: the open
/// issues among them.
pub fn walk_from(named: &[Named]) -> Vec<u64> {
    let mut open = Vec::new();
    for named in named {
        if let Some(read) = &named.read
            && !read.closed
            && !read.pull_request
        {
            open.push(named.number);
        }
    }
    open
}

/// Why a walk of the dependency chain stopped short.
#[derive(Debug, PartialEq, Eq)]
pub enum WalkError<E> {
    /// An issue's blockers could not be read.
    Read(E),
    /// The chain holds more than `WALK_LIMIT` issues.
    TooLong,
}

/// The issues a walk has read, against its limit.
#[refined_by(read: int)]
struct Visits {
    #[field(usize[read])]
    read: usize,
}

impl Visits {
    #[spec(fn() -> Visits[0])]
    fn none() -> Visits {
        Visits { read: 0 }
    }

    #[spec(fn(&Visits[@v]) -> bool[v.read < WALK_LIMIT])]
    fn has_room(&self) -> bool {
        self.read < WALK_LIMIT
    }

    /// One more issue is read only under the limit.
    #[spec(fn(self: &mut Self[@v]) requires v.read < WALK_LIMIT ensures self: Self[v.read + 1])]
    fn one_more(&mut self) {
        self.read += 1;
    }
}

/// Reads the chain of open blockers behind `from`, never past `issue` itself
/// and never an issue twice. `open_blockers_of` reads one issue's.
pub fn walk<E, F>(
    issue: u64,
    from: &[u64],
    mut open_blockers_of: F,
) -> Result<Vec<Edge>, WalkError<E>>
where
    F: FnMut(u64) -> Result<Vec<u64>, E>,
{
    let mut edges: Vec<Edge> = Vec::new();
    let mut visits = Visits::none();
    let mut todo: Vec<u64> = from.to_vec();
    while let Some(next) = todo.pop() {
        if next == issue || edges.iter().any(|edge| edge.issue == next) {
            continue;
        }
        if !visits.has_room() {
            return Err(WalkError::TooLong);
        }
        visits.one_more();
        let open_blockers = match open_blockers_of(next) {
            Ok(open_blockers) => open_blockers,
            Err(e) => return Err(WalkError::Read(e)),
        };
        todo.extend(open_blockers.iter().copied());
        edges.push(Edge { issue: next, open_blockers });
    }
    Ok(edges)
}

/// What holds a parked issue off the frontier first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The edge to this open blocker.
    Issue(u64),
    /// The label that says it waits on the next cut (`cut::WAITING_LABEL`).
    Cut,
}

/// The proof that a claim can be parked: every blocker its worker named is
/// closed or waiting, and at least one is still open or it waits on a cut
/// that will come — so what is written for it holds the issue off the
/// frontier once the claim is released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parkable {
    first: Hold,
    rest: Vec<u64>,
}

impl Parkable {
    /// The open blockers to wait on, in the order the worker named them.
    pub fn blockers(&self) -> Vec<u64> {
        let mut blockers = match self.first {
            Hold::Issue(first) => vec![first],
            Hold::Cut => vec![],
        };
        blockers.extend(self.rest.iter().copied());
        blockers
    }

    /// Whether it waits on the next cut of its repo.
    pub fn cut(&self) -> bool {
        self.first == Hold::Cut
    }

    pub(crate) fn first(&self) -> Hold {
        self.first
    }

    pub(crate) fn rest(&self) -> &[u64] {
        &self.rest
    }
}

/// What a `blocked` verdict comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// Every blocker it named is closed: the dependency cleared, and the
    /// issue is worked again.
    Redispatch,
    Park(Parkable),
    /// Nothing will ever clear it, and why.
    Escalate(String),
}

/// Routes a `blocked` verdict from the standing of each blocker it names, in
/// the order named, and from whether it waits on a cut and one will come.
pub fn blocked_route(standings: &[(u64, Standing)], cut: &CutWait) -> Blocked {
    if let CutWait::Never(reason) = cut {
        return Blocked::Escalate(format!(
            "The worker declared `blocked` on the next cut, and none will come — {reason}"
        ));
    }
    if standings.is_empty() && *cut == CutWait::No {
        return Blocked::Escalate("The worker declared `blocked` and named no blocker".to_string());
    }
    let mut unmet: Vec<String> = Vec::new();
    let mut waiting: Vec<u64> = Vec::new();
    for (number, standing) in standings {
        match standing {
            Standing::Closed => {}
            Standing::Waiting => {
                if !waiting.contains(number) {
                    waiting.push(*number);
                }
            }
            Standing::Unmet(reason) => unmet.push(format!("#{number} {reason}")),
        }
    }
    if !unmet.is_empty() {
        return Blocked::Escalate(format!(
            "The worker declared `blocked`, and nothing will resolve what it waits on — {}",
            unmet.join("; ")
        ));
    }
    if *cut == CutWait::Coming {
        return Blocked::Park(Parkable { first: Hold::Cut, rest: waiting });
    }
    let mut waiting = waiting.into_iter();
    match waiting.next() {
        Some(first) => {
            Blocked::Park(Parkable { first: Hold::Issue(first), rest: waiting.collect() })
        }
        None => Blocked::Redispatch,
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A dependency chain read past the limit.
    #[should_fail]
    fn a_walk_past_its_limit(visits: &mut Visits) {
        visits.one_more();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    fn config() -> Config {
        Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            ..Settings::default()
        })
        .unwrap()
    }

    fn open(number: u64, labels: &[&str]) -> Named {
        Named {
            number,
            read: Some(BlockerRead {
                closed: false,
                state_reason: None,
                labels: labels.iter().map(|label| label.to_string()).collect(),
                pull_request: false,
            }),
            claimed: false,
            open_pr: false,
        }
    }

    fn closed(number: u64, reason: &str) -> Named {
        let mut named = open(number, &[]);
        let read = named.read.as_mut().unwrap();
        read.closed = true;
        read.state_reason = Some(reason.to_string());
        named
    }

    fn unmet(named: &Named, edges: &[Edge]) -> String {
        match standing(7, named, &config(), &FlowLabels::default(), edges) {
            Standing::Unmet(reason) => reason,
            other => panic!("{other:?}"),
        }
    }

    fn edge(issue: u64, open_blockers: &[u64]) -> Edge {
        Edge { issue, open_blockers: open_blockers.to_vec() }
    }

    #[test]
    fn a_blocker_the_backlog_will_resolve_is_waiting() {
        let config = config();
        assert_eq!(
            standing(7, &open(41, &["ready-for-agent"]), &config, &FlowLabels::default(), &[]),
            Standing::Waiting
        );
        let claimed = Named { claimed: true, ..open(41, &[]) };
        assert_eq!(standing(7, &claimed, &config, &FlowLabels::default(), &[]), Standing::Waiting);
        let with_pr = Named { open_pr: true, ..open(41, &["bug"]) };
        assert_eq!(standing(7, &with_pr, &config, &FlowLabels::default(), &[]), Standing::Waiting);
        assert_eq!(
            standing(7, &closed(41, "completed"), &config, &FlowLabels::default(), &[]),
            Standing::Closed
        );
    }

    #[test]
    fn a_blocker_nothing_will_resolve_is_unmet() {
        let unreadable = Named { number: 41, read: None, claimed: true, open_pr: true };
        assert!(unmet(&unreadable, &[]).contains("could not be read"));
        let mut pr = open(41, &["ready-for-agent"]);
        pr.read.as_mut().unwrap().pull_request = true;
        assert!(unmet(&pr, &[]).contains("pull request"));
        assert_eq!(unmet(&closed(41, "not_planned"), &[]), "was closed as not planned");
        assert_eq!(unmet(&closed(41, "duplicate"), &[]), "was closed as a duplicate");
        // an epic is never dispatched, whoever claims it
        let epic = Named { claimed: true, ..open(41, &["ready-for-agent", "epic"]) };
        assert_eq!(unmet(&epic, &[]), "will never be dispatched: epic label (epic)");
        let epic = Named { open_pr: true, ..open(41, &["epic"]) };
        assert!(unmet(&epic, &[]).contains("epic label"));
        assert_eq!(
            unmet(&open(41, &["bug"]), &[]),
            "is open but no fleet will work it (unclaimed, no PR, no ready-for-agent label)"
        );
        assert_eq!(unmet(&open(7, &["ready-for-agent"]), &[]), "is the issue itself");
    }

    #[test]
    fn a_blocker_that_depends_on_the_blocked_issue_is_unmet() {
        let ready = open(41, &["ready-for-agent"]);
        let direct = [edge(41, &[7])];
        assert!(unmet(&ready, &direct).contains("already depends on #7"));
        let chain = [edge(41, &[50, 51]), edge(50, &[]), edge(51, &[60]), edge(60, &[7])];
        assert!(unmet(&ready, &chain).contains("close a cycle"));
        // a cycle that does not pass through the blocked issue is somebody else's
        let elsewhere = [edge(41, &[50]), edge(50, &[41])];
        assert_eq!(
            standing(7, &ready, &config(), &FlowLabels::default(), &elsewhere),
            Standing::Waiting
        );
        assert!(!depends_on(41, 7, &elsewhere));
        // a closed blocker's edges are not asked about
        assert_eq!(
            standing(7, &closed(41, "completed"), &config(), &FlowLabels::default(), &direct),
            Standing::Closed
        );
    }

    #[test]
    fn the_walk_reads_each_open_blocker_once_and_stops_at_the_issue() {
        let named = [open(41, &[]), closed(42, "completed"), open(43, &[])];
        assert_eq!(walk_from(&named), [41, 43]);
        let mut asked = vec![];
        let edges: Result<_, WalkError<()>> = walk(7, &[41, 43], |issue| {
            asked.push(issue);
            Ok(match issue {
                41 => vec![50, 43],
                43 => vec![7, 50],
                _ => vec![],
            })
        });
        let edges = edges.unwrap();
        asked.sort();
        assert_eq!(asked, [41, 43, 50], "7 itself is never read, and nothing twice");
        assert!(depends_on(41, 7, &edges));

        let failed = walk(7, &[41], |_| Err::<Vec<u64>, _>("gh down"));
        assert_eq!(failed, Err(WalkError::Read("gh down")));
        let endless = walk(7, &[100], |issue| Ok::<_, ()>(vec![issue + 1]));
        assert_eq!(endless, Err(WalkError::TooLong));
    }

    #[test]
    fn a_blocked_verdict_is_routed_by_its_blockers() {
        let no = CutWait::No;
        let unmet = Standing::Unmet("was closed as not planned".to_string());
        assert_eq!(
            blocked_route(&[(41, Standing::Closed), (42, Standing::Closed)], &no),
            Blocked::Redispatch
        );
        let waiting = [
            (41, Standing::Waiting),
            (42, Standing::Closed),
            (43, Standing::Waiting),
            (41, Standing::Waiting),
        ];
        let Blocked::Park(parkable) = blocked_route(&waiting, &no) else {
            panic!("every open blocker is waiting")
        };
        assert_eq!(parkable.blockers(), [41, 43], "the open ones, once each");
        assert!(!parkable.cut());
        let Blocked::Escalate(reason) =
            blocked_route(&[(41, Standing::Waiting), (42, unmet.clone()), (43, unmet)], &no)
        else {
            panic!("one unmet blocker escalates")
        };
        assert!(reason.ends_with("#42 was closed as not planned; #43 was closed as not planned"));
        let Blocked::Escalate(reason) = blocked_route(&[], &no) else { panic!() };
        assert!(reason.contains("named no blocker"));
    }

    #[test]
    fn a_verdict_on_the_next_cut_parks_while_a_cut_will_come() {
        let coming = CutWait::Coming;
        let Blocked::Park(parkable) = blocked_route(&[], &coming) else { panic!() };
        assert!(parkable.cut());
        assert_eq!(parkable.blockers(), Vec::<u64>::new());
        // closed blockers beside it clear nothing of the cut
        let Blocked::Park(parkable) =
            blocked_route(&[(41, Standing::Closed), (42, Standing::Waiting)], &coming)
        else {
            panic!()
        };
        assert!(parkable.cut());
        assert_eq!(parkable.blockers(), [42]);
        let never = CutWait::Never("the repo names no cut steps".to_string());
        let Blocked::Escalate(reason) = blocked_route(&[], &never) else { panic!() };
        assert!(reason.ends_with("none will come — the repo names no cut steps"), "{reason}");
    }

    #[test]
    fn what_a_blocked_verdict_names_reads_back_from_its_word() {
        let on = BlockedOn::read(Some("41, #42,cut,nonsense"));
        assert_eq!(on, BlockedOn { issues: vec![41, 42], cut: true });
        assert_eq!(on.word(), "41,42,cut");
        assert_eq!(BlockedOn::read(Some(&on.word())), on);
        assert_eq!(BlockedOn::read(Some("cut")).word(), "cut");
        assert!(BlockedOn::read(None).is_empty());
        assert_eq!(BlockedOn::default().word(), "");
    }
}

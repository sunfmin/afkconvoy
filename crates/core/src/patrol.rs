//! Patrols (docs/adr/0008): the schedule a role is run on for no issue, where
//! a run's claim and the record of the last run are kept on the remote, how
//! each patrol a repo names stands at one moment, and what makes a run a
//! failed one.

use flux_rs::attrs::*;
use serde::Serialize;

use crate::claims::{Holder, InstanceId, RunClaim};
use crate::layout::Layout;
use crate::lease::{SKEW, stamp_age};
use crate::plan::Issue;
use crate::record;
use crate::worker::{GRACE, WorkerState};

/// The kind of ref the record of a patrol's last run is kept under, named for
/// the patrol.
pub const RUN_KIND: &str = "patrol/";
/// What the claim on a patrol's run is keyed by: this, then the patrol's name.
const CLAIM_KEY: &str = "patrol-";

const RUN: &str = "convoy:patrol-run";
/// The kind of record the report of a failed run carries.
pub const FAILED: &str = "convoy:patrol-failed";

/// The variable a patrol's worker carries the patrol's name in: the command
/// that takes an issue in reads it.
pub const PATROL_ENV: &str = "AFKCONVOY_PATROL";

/// The ref that records when the patrol `name` last ran.
pub fn run_ref(layout: Layout, name: &str) -> String {
    layout.under(&format!("{RUN_KIND}{name}"))
}

/// The key of the claim on a run of the patrol `name` (`claims::RunClaim`).
pub fn claim_key(name: &str) -> String {
    format!("{CLAIM_KEY}{name}")
}

/// The patrol a run's claim keyed `key` is on a run of; `None` for a claim on
/// anything else.
pub fn claimed(key: &str) -> Option<&str> {
    key.strip_prefix(CLAIM_KEY)
}

/// The window `max_runs_a_day` counts a patrol's runs in, in seconds: the
/// last 24 hours, rolling.
pub const DAY: u64 = 86_400;

/// What the record of a patrol's last run says: when it started, and when
/// the runs before it that the day before it holds did. Each run's record
/// carries the day of the one before, so the runs of a day are read off the
/// one ref the last run is recorded under.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ran {
    /// When the run started.
    pub ts: u64,
    /// When each earlier run of the day before `ts` started, oldest first.
    pub before: Vec<u64>,
}

impl Ran {
    /// The run started at `now`, after the run `last` recorded: it carries
    /// every start of `last` the day before `now` still holds.
    pub fn after(last: Option<&Ran>, now: u64) -> Ran {
        let mut before: Vec<u64> = match last {
            Some(last) => last.before.iter().copied().chain([last.ts]).collect(),
            None => vec![],
        };
        before.retain(|start| in_day(*start, now));
        Ran { ts: now, before }
    }

    /// The starts it records that the day before `now` holds, oldest first.
    pub fn day(&self, now: u64) -> Vec<u64> {
        let mut day: Vec<u64> = self.before.iter().copied().chain([self.ts]).collect();
        day.retain(|start| in_day(*start, now));
        day.sort_unstable();
        day
    }
}

/// Whether a run started at `start` is one of the day before `now`: less
/// than `DAY` old, and no further ahead of `now` than `SKEW`.
fn in_day(start: u64, now: u64) -> bool {
    if start <= now { now - start < DAY } else { start - now <= SKEW }
}

/// The record of the run `ran`.
pub fn write_run(ran: &Ran) -> String {
    let before: Vec<String> = ran.before.iter().map(u64::to_string).collect();
    record::write(RUN, &[("ts", &ran.ts.to_string()), ("before", &before.join(","))])
}

/// A commit subject → the run it records.
pub fn read_run(subject: &str) -> Option<Ran> {
    let fields = record::read(RUN, subject)?;
    Some(Ran { ts: fields.number("ts")?, before: fields.numbers("before") })
}

/// How many runs a day a patrol run every `every` seconds has at most: as
/// many as fit in `DAY` that far apart. A `max_runs_a_day` of as many or more
/// bounds nothing.
pub fn runs_a_day(every: u64) -> u64 {
    if every == 0 { u64::MAX } else { DAY / every + u64::from(DAY % every != 0) }
}

/// Until when a patrol that may run `max` times a day, whose last recorded
/// run is `last`, is held by that bound at `now`: the moment enough of the
/// runs of the day before `now` are a day old that fewer than `max` are left.
/// `None` when it is not held: it has no bound, or ran fewer times.
pub fn held_until(max: Option<u64>, last: Option<&Ran>, now: u64) -> Option<u64> {
    let (max, last) = (max?, last?);
    let day = last.day(now);
    let over = (day.len() as u64).checked_sub(max)?;
    let oldest = day.get(usize::try_from(over).ok()?)?;
    Some(oldest.saturating_add(DAY))
}

/// Whether a patrol run every `every` seconds, whose last run started at
/// `stamp`, is due at `now`: the interval has passed. A stamp further ahead
/// of `now` than `SKEW` is no evidence of a run, and the patrol is due.
#[spec(fn(every: u64, stamp: u64, now: u64) -> bool[
    (stamp > now && stamp - now > SKEW) || (stamp <= now && now - stamp >= every)])]
pub fn interval_passed(every: u64, stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp >= every } else { stamp - now > SKEW }
}

/// Whether a patrol is due: it never ran, or `interval_passed`. One run makes
/// it not due for a whole interval, however many it missed before: a missed
/// run is not made up.
pub fn is_due(every: u64, last_run: Option<u64>, now: u64) -> bool {
    match last_run {
        Some(stamp) => interval_passed(every, stamp, now),
        None => true,
    }
}

/// What the effect layer read of one patrol the repo names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub name: String,
    /// Its interval, in seconds.
    pub every: u64,
    /// How many runs the day before may hold, when it says.
    pub max_runs_a_day: Option<u64>,
    pub max_open: u64,
    /// Its last recorded run.
    pub last_run: Option<Ran>,
    /// The claim on its run, while one stands.
    pub claim: Option<RunClaim>,
    /// How many open issues it opened.
    pub open: u64,
    /// The open issue that reports its last run as failed, while there is one.
    pub failure: Option<u64>,
}

/// How one patrol stands: what a tick does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// Its interval has passed and nothing holds it back: a tick runs it.
    Due,
    /// Its interval has not passed since its last recorded run.
    NotDue,
    /// The runs of the last `DAY` have reached its `max_runs_a_day`.
    Spent,
    /// A fleet lock stands: no patrol runs.
    Locked,
    /// The open issues it opened have reached its `max_open`.
    Full,
    /// Its last run failed, and the issue that reports it is still open.
    Failed,
    /// A run of mine is still going: a tick asks after its worker.
    Running,
    /// A live peer's run is still going.
    PeerRunning,
    /// The claim on its run is a peer's presumed dead: a tick deletes it.
    StaleRun,
}

/// One patrol the repo names, as a working set holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub name: String,
    pub standing: Standing,
    pub every: u64,
    pub last_run: Option<u64>,
    pub open: u64,
    pub max_open: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_runs_a_day: Option<u64>,
    /// How many of its runs started in the last `DAY`, where it has a
    /// `max_runs_a_day` to hold them against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs_in_day: Option<u64>,
    /// Until when `max_runs_a_day` holds it: `Spent` in this case only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<u64>,
    /// The claim on its run, while one stands.
    #[serde(skip)]
    pub claim: Option<RunClaim>,
}

/// How the patrol `seen` stands for the fleet instance `me` at `now`, with
/// each instance's heartbeat in `heartbeats`, and `locked` when a fleet lock
/// stands. A run still going comes first — whoever holds it, the patrol is
/// skipped, however long it has been due; then the failure nobody answered,
/// the interval, the runs of the day, the lock, and the limit on what it
/// opened.
pub fn row(
    seen: &Seen,
    me: Option<&InstanceId>,
    heartbeats: &[(String, u64)],
    now: u64,
    locked: bool,
) -> Row {
    let last = seen.last_run.as_ref();
    let until = held_until(seen.max_runs_a_day, last, now);
    let standing = match &seen.claim {
        Some(claim) => match claim.holder(me, heartbeats, now) {
            Holder::Mine => Standing::Running,
            Holder::Dead => Standing::StaleRun,
            Holder::Peer => Standing::PeerRunning,
        },
        None if seen.failure.is_some() => Standing::Failed,
        None if !is_due(seen.every, last.map(|ran| ran.ts), now) => Standing::NotDue,
        None if until.is_some() => Standing::Spent,
        None if locked => Standing::Locked,
        None if seen.open >= seen.max_open => Standing::Full,
        None => Standing::Due,
    };
    Row {
        name: seen.name.clone(),
        standing,
        every: seen.every,
        last_run: last.map(|ran| ran.ts),
        open: seen.open,
        max_open: seen.max_open,
        max_runs_a_day: seen.max_runs_a_day,
        runs_in_day: seen.max_runs_a_day.map(|_| last.map_or(0, |ran| ran.day(now).len() as u64)),
        until: until.filter(|_| standing == Standing::Spent),
        failure: seen.failure,
        claim: seen.claim.clone(),
    }
}

/// Whether the run `claim` is of — a patrol's, a proposal's — is over, its
/// worker being in `worker`: the worker declared it over, or is gone, or
/// stopped for a grace period. A worker that never declares is one that is
/// done once it has stopped that long.
pub fn run_over(claim: &RunClaim, worker: WorkerState, now: u64) -> bool {
    if claim.over {
        return true;
    }
    let started = claim.owner.as_ref().map(|owner| owner.ts);
    match worker {
        WorkerState::Gone => true,
        WorkerState::Busy => false,
        WorkerState::Idle { quiet: Some(quiet) } => quiet >= GRACE,
        // Neither a report nor the terminal says since when: the run's own age decides.
        WorkerState::Idle { quiet: None } => match started {
            Some(started) => stamp_age(started, now).is_none_or(|age| age >= GRACE),
            None => true,
        },
    }
}

/// Why a run failed, when it did: its checkout, at `started` when its worker
/// was started, is at `tip` now — it left a commit — or it opened the PRs
/// `prs`. A patrol writes no code; every change it prompts goes through a
/// workflow.
pub fn broke(started: Option<&str>, tip: &str, prs: &[u64]) -> Option<String> {
    let mut found: Vec<String> = vec![];
    if let Some(started) = started.filter(|started| *started != tip) {
        found.push(format!("it left a commit in its checkout ({started} → {tip})"));
    }
    if !prs.is_empty() {
        let numbers: Vec<String> = prs.iter().map(|pr| format!("#{pr}")).collect();
        found.push(format!("it opened {}", numbers.join(", ")));
    }
    if found.is_empty() { None } else { Some(found.join(", and ")) }
}

/// The title of the issue that reports a failed run of the patrol `name`: it
/// is how the report is found again.
pub fn failure_title(name: &str) -> String {
    format!("afkconvoy: the patrol `{name}` failed")
}

/// The body of that issue: the record, why the run failed, and where what it
/// left is.
pub fn failure_report(name: &str, reason: &str, host: &str, checkout: &str) -> String {
    let marker = record::marker(FAILED, &[("patrol", name)]);
    format!(
        "{marker}\n**afkconvoy** — a run of the patrol `{name}` failed: {reason}. A patrol opens \
         issues and takes them in; it writes no code.\n\nNothing was undone: what it left is in \
         its checkout `{checkout}` on the host `{host}`, and any PR it opened is still open. The \
         patrol is not run again while this issue is open — close it once you have looked."
    )
}

/// The open issue among `issues` that reports a failed run of the patrol
/// `name`: one titled `failure_title` that carries `escalate_label`.
pub fn failure_of(issues: &[Issue], name: &str, escalate_label: &str) -> Option<u64> {
    let title = failure_title(name);
    let mut found: Option<u64> = None;
    for issue in issues {
        let reports = issue.title == title && issue.labels.iter().any(|l| l == escalate_label);
        if reports && found.is_none_or(|first| issue.number < first) {
            found = Some(issue.number);
        }
    }
    found
}

/// The name a patrol's checkout is created under: one per patrol on a
/// machine at a time, cut for a run and removed as it ends.
pub fn checkout_name(name: &str) -> String {
    format!("convoy-patrol-{name}")
}

/// What callers may rely on: each is checked by Flux, and stops passing when
/// the spec it reads is weakened.
#[cfg(flux)]
#[allow(dead_code)]
mod relied_on {
    use super::*;

    /// A patrol that ran is not due again until its interval has passed.
    #[spec(fn(every: u64, ran: u64, later: u64) requires ran <= later && later - ran < every)]
    fn a_run_is_not_due_within_its_interval(every: u64, ran: u64, later: u64) {
        flux_rs::assert(!interval_passed(every, ran, later));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;
    use crate::claims::{over_ref, read_over, run_ref as claim_ref, write_over};
    use crate::lease::LEASE;

    fn seen() -> Seen {
        Seen {
            name: "triage".to_string(),
            every: 3600,
            max_runs_a_day: None,
            max_open: 2,
            last_run: Some(ran(10_000, &[])),
            claim: None,
            open: 0,
            failure: None,
        }
    }

    fn ran(ts: u64, before: &[u64]) -> Ran {
        Ran { ts, before: before.to_vec() }
    }

    fn claim(instance: &str) -> RunClaim {
        let owner = Owner { instance: instance.into(), host: "h".into(), ts: 0 };
        RunClaim { key: claim_key("triage"), owner: Some(owner), sha: "c".into(), over: false }
    }

    fn standing(seen: &Seen, now: u64, locked: bool) -> Standing {
        let me = InstanceId::parse("me").unwrap();
        row(seen, Some(&me), &[("peer".into(), now)], now, locked).standing
    }

    #[test]
    fn a_patrol_is_due_once_its_interval_has_passed_since_its_last_run() {
        assert!(is_due(3600, None, 5), "one that never ran");
        assert!(!is_due(3600, Some(10_000), 13_599));
        assert!(is_due(3600, Some(10_000), 13_600));
        assert!(!is_due(3600, Some(10_100), 10_000), "skew reads as just now");
        assert!(is_due(3600, Some(10_301), 10_000), "a stamp from the future is no run");
    }

    #[test]
    fn a_missed_run_is_not_made_up() {
        // Ten intervals late, it runs once: the run recorded then is the last.
        assert!(is_due(3600, Some(10_000), 46_000));
        assert!(!is_due(3600, Some(46_000), 46_001));
    }

    #[test]
    fn a_patrol_stands_by_what_holds_it_back_first() {
        assert_eq!(standing(&seen(), 13_600, false), Standing::Due);
        assert_eq!(standing(&seen(), 13_599, false), Standing::NotDue);
        assert_eq!(standing(&seen(), 13_600, true), Standing::Locked);
        assert_eq!(standing(&Seen { open: 2, ..seen() }, 13_600, false), Standing::Full);
        assert_eq!(standing(&Seen { open: 1, ..seen() }, 13_600, false), Standing::Due);
        let failed = Seen { failure: Some(9), ..seen() };
        assert_eq!(standing(&failed, 13_600, false), Standing::Failed);
    }

    #[test]
    fn a_runs_record_carries_the_starts_of_the_day_before_it() {
        let first = Ran::after(None, 1000);
        assert_eq!(first, ran(1000, &[]));
        let second = Ran::after(Some(&first), 5000);
        assert_eq!(second, ran(5000, &[1000]));
        // A start a day old is no longer carried.
        let third = Ran::after(Some(&second), 1000 + DAY);
        assert_eq!(third, ran(1000 + DAY, &[5000]));
        assert_eq!(third.day(5000 + DAY - 1), [5000, 1000 + DAY]);
        assert_eq!(third.day(5000 + DAY), [1000 + DAY]);
    }

    #[test]
    fn a_patrol_that_ran_its_max_runs_a_day_is_held_until_the_oldest_is_a_day_old() {
        let day = ran(30_000, &[10_000, 20_000]);
        assert_eq!(held_until(None, Some(&day), 30_001), None, "no bound");
        assert_eq!(held_until(Some(3), None, 30_001), None, "no run");
        assert_eq!(held_until(Some(4), Some(&day), 30_001), None, "fewer runs");
        assert_eq!(held_until(Some(3), Some(&day), 30_001), Some(10_000 + DAY));
        // One more than it may: held until the second oldest is a day old too.
        assert_eq!(held_until(Some(2), Some(&day), 30_001), Some(20_000 + DAY));
        assert_eq!(held_until(Some(3), Some(&day), 10_000 + DAY), None);

        let bounded = Seen { max_runs_a_day: Some(3), last_run: Some(day), ..seen() };
        let me = InstanceId::parse("me").unwrap();
        let held = row(&bounded, Some(&me), &[], 30_000 + 3600, false);
        assert_eq!((held.standing, held.until), (Standing::Spent, Some(10_000 + DAY)));
        assert_eq!(held.runs_in_day, Some(3));
        let due = row(&bounded, Some(&me), &[], 10_000 + DAY, false);
        assert_eq!((due.standing, due.until, due.runs_in_day), (Standing::Due, None, Some(2)));
        // The interval comes first: within it the patrol is not due, whatever its day.
        assert_eq!(standing(&bounded, 30_001, false), Standing::NotDue);
    }

    #[test]
    fn a_patrol_runs_at_most_as_many_times_a_day_as_its_interval_fits() {
        assert_eq!(runs_a_day(3600), 24);
        assert_eq!(runs_a_day(7 * 3600), 4);
        assert_eq!(runs_a_day(DAY), 1);
        assert_eq!(runs_a_day(2 * DAY), 1);
        assert_eq!(runs_a_day(60), 1440);
    }

    #[test]
    fn a_run_still_going_is_whose_its_claim_says() {
        let mine = Seen { claim: Some(claim("me")), ..seen() };
        assert_eq!(standing(&mine, 99_000, true), Standing::Running);
        let peer = Seen { claim: Some(claim("peer")), ..seen() };
        assert_eq!(standing(&peer, 99_000, false), Standing::PeerRunning);
        let me = InstanceId::parse("me").unwrap();
        let dead = row(&peer, Some(&me), &[("peer".into(), 1000)], 1001 + LEASE, false);
        assert_eq!(dead.standing, Standing::StaleRun);
        // With nobody's claims its own, a run is a peer's.
        assert_eq!(row(&mine, None, &[("me".into(), 5)], 5, false).standing, Standing::PeerRunning);
        let nobody = RunClaim { owner: None, ..claim("x") };
        let unowned = Seen { claim: Some(nobody), ..seen() };
        assert_eq!(standing(&unowned, 5, false), Standing::StaleRun);
    }

    #[test]
    fn a_run_is_over_when_its_worker_is_gone_or_stopped_for_a_grace_period() {
        let started = |ts: u64| {
            let owner = Owner { instance: "me".into(), host: "h".into(), ts };
            RunClaim { owner: Some(owner), ..claim("me") }
        };
        assert!(run_over(&started(0), WorkerState::Gone, 10));
        assert!(!run_over(&started(0), WorkerState::Busy, 99_999));
        assert!(!run_over(&started(0), WorkerState::Idle { quiet: Some(GRACE - 1) }, 99_999));
        assert!(run_over(&started(0), WorkerState::Idle { quiet: Some(GRACE) }, 99_999));
        let silent = WorkerState::Idle { quiet: None };
        assert!(!run_over(&started(1000), silent, 1000 + GRACE - 1));
        assert!(run_over(&started(1000), silent, 1000 + GRACE));
        assert!(run_over(&RunClaim { owner: None, ..claim("me") }, silent, 5));
    }

    #[test]
    fn a_run_its_worker_declared_over_is_over_whatever_the_worker_is_doing() {
        let declared = RunClaim { over: true, ..claim("me") };
        assert!(run_over(&declared, WorkerState::Busy, 1));
        assert!(run_over(&declared, WorkerState::Idle { quiet: Some(0) }, 1));
        // The record names the claim it is of, under the name the claim has.
        assert_eq!(
            over_ref(Layout::Hidden, &claim_key("triage")),
            "refs/convoy/over/patrol-triage"
        );
        assert_eq!(read_over(&write_over("abc")), Some("abc".to_string()));
        assert_eq!(read_over("convoy:claim ts=77"), None);
    }

    #[test]
    fn a_run_that_left_a_commit_or_opened_a_pr_failed() {
        assert_eq!(broke(Some("a"), "a", &[]), None);
        assert_eq!(broke(None, "a", &[]), None);
        let commit = broke(Some("a"), "b", &[]).unwrap();
        assert_eq!(commit, "it left a commit in its checkout (a → b)");
        assert_eq!(broke(Some("a"), "a", &[4, 6]).unwrap(), "it opened #4, #6");
        assert!(broke(Some("a"), "b", &[4]).unwrap().ends_with(", and it opened #4"));
    }

    #[test]
    fn a_failure_is_found_again_by_its_title_and_the_escalate_label() {
        let issue = |number: u64, title: &str, labels: &[&str]| Issue {
            number,
            title: title.to_string(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            open_blockers: 0,
        };
        let title = failure_title("triage");
        let issues = [
            issue(9, &title, &["ready-for-human"]),
            issue(4, &title, &["bug"]),
            issue(7, &title, &["ready-for-human"]),
            issue(3, &failure_title("other"), &["ready-for-human"]),
        ];
        assert_eq!(failure_of(&issues, "triage", "ready-for-human"), Some(7));
        assert_eq!(failure_of(&issues, "sweep", "ready-for-human"), None);
        let report = failure_report("triage", "it opened #4", "host-1", "/wt/p");
        assert!(report.starts_with("<!--convoy:patrol-failed patrol=triage-->\n"), "{report}");
    }

    #[test]
    fn the_refs_of_a_patrol_are_named_for_it() {
        assert_eq!(run_ref(Layout::Hidden, "triage"), "refs/convoy/patrol/triage");
        let claim = claim_ref(Layout::Hidden, &claim_key("triage"));
        assert_eq!(claim, "refs/convoy/claim/patrol-triage");
        assert_eq!(read_run(&write_run(&ran(77, &[]))), Some(ran(77, &[])));
        assert_eq!(write_run(&ran(77, &[5, 9])), "convoy:patrol-run ts=77 before=5,9");
        assert_eq!(read_run(&write_run(&ran(77, &[5, 9]))), Some(ran(77, &[5, 9])));
        assert_eq!(read_run("convoy:claim ts=77"), None);
    }
}

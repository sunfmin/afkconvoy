//! The work account: the one comment an issue shows when work on it ends.
//! The fleet writes it and edits that same comment at each later end. A
//! worker does not. Position, rework and the guard do not read it.
//!
//! The total is the wall clock from the earliest claim `since` on record to
//! the end. The rows under it are not a partition of that total, and nothing
//! here adds them up.

use flux_rs::attrs::*;

mod data;
mod goal;
pub mod rewrite;
mod tokens;
pub use tokens::parse as parse_tokens;
pub use tokens::parse_between as parse_tokens_between;
pub use tokens::{Counts, Tokens};

pub use data::{Gate, Host, Record, SCHEMA_VERSION, Worker, read, render};
pub use goal::{CLEARED, WorkerGoal};

use crate::gate::GATE;
use crate::record;
use crate::stage_record::{Author, StageRecord, Stated};

/// The marker the one account comment opens with.
pub const ACCOUNT: &str = "convoy:account";
/// The marker the fleet posts when it starts a worker that is its own row.
pub const WORKER: &str = "convoy:worker";
/// A red gate run cited by the account, kept in the comment after its ref is gone.
pub const KEPT_RED: &str = "convoy:account-red";

/// How work on an issue ends, and so whether that end leaves the work
/// account (`accounted`). The transitions that end a claim take theirs, and
/// cannot release it before an end that accounts has left the account. A new
/// kind of end is written here, and gets its account by being one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(accounted: bool)]
pub enum End {
    /// Its pull request landed on the base branch.
    #[variant(End[true])]
    Landed,
    /// Closed with nothing more to do: no change was needed, or a consult
    /// closed it.
    #[variant(End[true])]
    Closed,
    /// Handed to a human.
    #[variant(End[true])]
    Escalated,
    /// Its workflow ended.
    #[variant(End[true])]
    Ended,
    /// A human closed it while a claim was held.
    #[variant(End[true])]
    HumanClosed,
    /// Parked on its blockers: work goes on once they close.
    #[variant(End[false])]
    Parked,
    /// Handed on to a later stage, or back to the frontier.
    #[variant(End[false])]
    HandedOn,
    /// The issue is gone: nothing is left to write an account on.
    #[variant(End[false])]
    Forgotten,
}

impl End {
    /// Whether this end leaves the work account.
    #[spec(fn(&Self[@end]) -> bool[end])]
    pub fn accounted(&self) -> bool {
        match self {
            End::Landed | End::Closed | End::Escalated | End::Ended | End::HumanClosed => true,
            End::Parked | End::HandedOn | End::Forgotten => false,
        }
    }
}

/// Whether an issue's comments already carry its work account.
pub fn left<'a>(bodies: impl IntoIterator<Item = &'a str>) -> bool {
    bodies.into_iter().any(|body| record::read_marker(ACCOUNT, body).is_some())
}

/// The end settling a claim whose work is on the base branch leaves the
/// account for, or `None` when it leaves none. A landing turn that already
/// left the account was that end, and settling it does not move the end. A
/// claim still held on an issue a human closed, or a landing cut short of its
/// account, leaves one in settling.
pub fn settled_end(landed: bool, accounted: bool) -> Option<End> {
    match (landed, accounted) {
        (true, true) => None,
        (true, false) => Some(End::Landed),
        (false, _) => Some(End::HumanClosed),
    }
}

/// One comment the fleet has already read, oldest first.
pub struct Heard<'a> {
    /// When the comment was written, in epoch seconds. Missing is not a time.
    pub at: Option<u64>,
    pub body: &'a str,
}

/// The host that is writing the account, as that host reports it.
#[derive(Clone, Copy)]
pub struct Machine<'a> {
    /// The chip name, when the host reports one.
    pub chip: Option<&'a str>,
    /// The model identifier, used when the host reports no chip name.
    pub model: Option<&'a str>,
    /// The logical CPU count, when the host reports one.
    pub cpus: Option<u64>,
    /// The memory size in bytes, when the host reports one.
    pub bytes: Option<u64>,
}

/// A worker the fleet has just started, and that is its own row.
pub struct Begun<'a> {
    pub at: u64,
    /// The claim it was started under (`Claim::id`).
    pub claim: &'a str,
    /// That claim's `since`, so the earliest one survives the claim's deletion.
    pub since: Option<u64>,
    pub harness: &'a str,
    pub model: Option<&'a str>,
    /// The stage, for a bound repo. None for an unbound repo.
    pub stage: Option<&'a str>,
    pub role: Option<&'a str>,
    pub consult: bool,
}

/// Whether this start is a new row. A landing worker is not a row. A
/// continuation or a takeover of the same harness is the same worker.
pub fn own_row(landing: bool, continued_from: Option<&str>, harness: &str) -> bool {
    !landing && continued_from != Some(harness)
}

/// Whether a worker start for `claim` on `harness` is already in `bodies`.
/// A continuation or a takeover of that worker is not another row. An
/// account quotes those markers, and a quotation is not a start.
pub fn already_noted<'a>(
    bodies: impl IntoIterator<Item = &'a str>,
    claim: &str,
    harness: &str,
) -> bool {
    bodies.into_iter().any(|body| {
        if record::read_marker(ACCOUNT, body).is_some() {
            return false;
        }
        let Some(fields) = record::read_marker(WORKER, body) else { return false };
        fields.get("claim") == Some(claim) && fields.get("harness") == Some(harness)
    })
}

/// The comment posted when the fleet starts a worker that [`own_row`] names.
pub fn worker_comment(begun: &Begun<'_>) -> String {
    let at = begun.at.to_string();
    let since = begun.since.map(|since| since.to_string()).unwrap_or_default();
    let marker = record::marker(
        WORKER,
        &[
            ("at", &at),
            ("claim", begun.claim),
            ("since", &since),
            ("harness", begun.harness),
            ("model", begun.model.unwrap_or("")),
            ("stage", begun.stage.unwrap_or("")),
            ("role", begun.role.unwrap_or("")),
            ("consult", if begun.consult { "1" } else { "" }),
        ],
    );
    format!("{marker}\n**afkconvoy** — worker started.")
}

/// What the fleet has already read, and the moment the account is written.
pub struct Told<'a> {
    pub issue: u64,
    pub repo: &'a str,
    pub tracker: &'a str,
    pub version: &'a str,
    pub outcome: End,
    /// Known commands and their lanes from the writing fleet's config.
    pub lanes: &'a [(&'a str, &'a str)],
    pub heard: &'a [Heard<'a>],
    /// Subjects of the gate refs the remote holds.
    pub gates: &'a [&'a str],
    /// Whether a claim ref is still held.
    pub claimed: bool,
    /// That claim's `since`, when the record states one.
    pub held_since: Option<u64>,
    pub end: u64,
    pub machine: Machine<'a>,
    /// The host writing the account, as `gethostname` spells it.
    pub host: &'a str,
}

/// The account comment to leave, or `None` when the issue never had a claim:
/// nothing is claimed now, no worker was started, and no account is there yet.
pub fn comment(told: &Told<'_>) -> Option<String> {
    let prior = prior_account(told.heard, told.issue);
    if prior.is_none() && left(told.heard.iter().map(|note| note.body)) {
        return None;
    }
    let starts = worker_starts(told.heard);
    if prior.is_none() && starts.is_empty() && !told.claimed {
        return None;
    }
    let mut sinces: Vec<u64> = starts.iter().filter_map(|start| start.since).collect();
    if let Some(since) = told.held_since {
        sinces.push(since);
    }
    let start = kept_start(prior.as_ref(), &sinces);
    let kept = prior.as_ref().map(|prior| prior.reds.as_slice()).unwrap_or(&[]);
    let row = Record {
        schema_version: SCHEMA_VERSION,
        legacy_evidence: prior.as_ref().and_then(|p| p.legacy_evidence.clone()),
        issue: told.issue,
        repo: Some(told.repo.to_string()),
        tracker: Some(told.tracker.to_string()),
        afkconvoy_version: Some(told.version.to_string()),
        host: Host {
            name: text(Some(told.host)).map(str::to_string),
            cpu: text(told.machine.chip).or_else(|| text(told.machine.model)).map(str::to_string),
            logical_cpus: told.machine.cpus,
            memory_bytes: told.machine.bytes,
        },
        start,
        end: Some(told.end),
        wall_seconds: start.and_then(|start| told.end.checked_sub(start)),
        outcome: Some(
            match told.outcome {
                End::Landed => "landed",
                End::Closed => "closed",
                End::Escalated => "escalated",
                End::Ended => "ended",
                End::HumanClosed => "human-closed",
                End::Parked => "parked",
                End::HandedOn => "handed-on",
                End::Forgotten => "forgotten",
            }
            .to_string(),
        ),
        workers: worker_rows(told.heard, &starts, told.end),
        gates: gate_rows(told.issue, told.gates, kept, told.lanes),
    };
    Some(render(&row))
}

struct Prior {
    legacy_evidence: Option<String>,
    start: Option<u64>,
    reds: Vec<Gate>,
}

struct Start {
    index: usize,
    at: u64,
    claim: String,
    since: Option<u64>,
    harness: String,
    model: Option<String>,
    stage: Option<String>,
    role: Option<String>,
}

fn prior_account(heard: &[Heard<'_>], issue: u64) -> Option<Prior> {
    let body = heard.iter().find(|note| record::read_marker(ACCOUNT, note.body).is_some())?;
    let row = read(body.body, issue).ok()?;
    Some(Prior {
        legacy_evidence: row.legacy_evidence,
        start: row.start,
        reds: row.gates.into_iter().filter(|gate| gate.result.as_deref() == Some("red")).collect(),
    })
}

/// The start already written, else the earliest `since` on record. A later
/// edit never moves a known start forward. An account written with no start
/// stays that way: a claim created after that end does not fill one in.
fn kept_start(prior: Option<&Prior>, sinces: &[u64]) -> Option<u64> {
    match prior {
        Some(prior) => prior.start.map(|start| sinces.iter().copied().fold(start, u64::min)),
        None => sinces.iter().copied().min(),
    }
}

fn worker_starts(heard: &[Heard<'_>]) -> Vec<Start> {
    let mut found = vec![];
    for (index, note) in heard.iter().enumerate() {
        // The account quotes a start's marker. That quotation is not a second start.
        if record::read_marker(ACCOUNT, note.body).is_some() {
            continue;
        }
        let Some(fields) = record::read_marker(WORKER, note.body) else { continue };
        if fields.get("landing").is_some() {
            continue;
        }
        let Some(at) = fields.number("at") else { continue };
        let Some(harness) = fields.get("harness") else { continue };
        let Some(claim) = fields.get("claim") else { continue };
        found.push(Start {
            index,
            at,
            claim: claim.to_string(),
            since: fields.number("since"),
            harness: harness.to_string(),
            model: fields.get("model").map(str::to_string),
            stage: fields.get("stage").map(str::to_string),
            role: fields.get("role").map(str::to_string),
        });
    }
    found
}

fn worker_rows(heard: &[Heard<'_>], starts: &[Start], end: u64) -> Vec<Worker> {
    let mut rows = vec![];
    let mut later = starts.iter().peekable();
    while let Some(start) = later.next() {
        let next = later.peek().map(|start| start.index).unwrap_or(heard.len());
        // The start's own comment is not in its window. A range that cannot
        // be is empty: the row is still written, with nothing cited.
        let from = start.index.saturating_add(1);
        let window: Vec<(Option<u64>, StageRecord)> = heard
            .get(from..next)
            .unwrap_or(&[])
            .iter()
            .filter_map(|note| {
                let record = StageRecord::read(note.body)?;
                Some((note.at, record))
            })
            .collect();
        let cited = window
            .iter()
            .rev()
            .find(|(_, record)| record.claim() == Some(start.claim.as_str()))
            .or_else(|| window.iter().rev().find(|(_, record)| fleet_stop(record)));
        rows.push(worker_row(start, cited.map(|(at, record)| cite(*at, record)), end));
    }
    rows
}

fn fleet_stop(record: &StageRecord) -> bool {
    matches!(record.by, Author::Fleet)
        && matches!(record.stated, Stated::Failed(_) | Stated::LockClosed)
}

struct Cite {
    at: Option<u64>,
    stage: String,
    verdict: String,
    harness: Option<String>,
    model: Option<String>,
}

fn cite(at: Option<u64>, record: &StageRecord) -> Cite {
    let (harness, model) = match &record.by {
        Author::Worker { harness, model, .. } => (Some(harness.key().to_string()), model.clone()),
        Author::Human | Author::Fleet => (None, None),
    };
    Cite {
        at,
        stage: record.stage.clone(),
        verdict: record.stated.word().to_string(),
        harness,
        model,
    }
}

fn worker_row(start: &Start, cited: Option<Cite>, end: u64) -> Worker {
    let harness = cited
        .as_ref()
        .and_then(|cite| cite.harness.clone())
        .unwrap_or_else(|| start.harness.clone());
    let model = cited.as_ref().and_then(|cite| cite.model.clone()).or_else(|| start.model.clone());
    let stage = cited.as_ref().map(|cite| cite.stage.clone()).or_else(|| start.stage.clone());
    let verdict = cited.as_ref().map(|cite| cite.verdict.clone());

    let cited_at = cited.as_ref().and_then(|cite| cite.at);
    // A bound row ends at the stage record or the failure. An unbound row
    // runs to the end of the issue, whatever a gate timed during it.
    let row_end = if start.stage.is_some() { cited_at.unwrap_or(end) } else { end };
    let seconds = row_end.checked_sub(start.at);
    Worker {
        harness: Some(harness),
        model,
        stage,
        verdict,
        role: start.role.clone(),
        claim: Some(start.claim.clone()),
        start: Some(start.at),
        end: Some(row_end),
        seconds,
        tokens: None,
        goal: None,
    }
}

fn gate_rows(issue: u64, gates: &[&str], kept: &[Gate], lanes: &[(&str, &str)]) -> Vec<Gate> {
    let mut rows = vec![];
    for subject in gates {
        let Some(fields) = record::read(GATE, subject) else { continue };
        if fields.number("issue") != Some(issue) {
            continue;
        }
        let result = if fields.get("red").is_some() { "red" } else { "green" };
        let mut run = data::gate_of(&fields, result);
        run.lane = lanes
            .iter()
            .find(|(command, _)| Some(*command) == run.command.as_deref())
            .map(|(_, lane)| lane.to_string());
        if !rows.iter().any(|have| same_run(have, &run)) {
            rows.push(run);
        }
    }
    for run in kept {
        if !rows.iter().any(|have| same_run(have, run)) {
            rows.push(run.clone());
        }
    }
    rows.sort_by_key(|run| (run.at.is_none(), run.at.unwrap_or(0)));
    rows
}

fn same_run(one: &Gate, other: &Gate) -> bool {
    one.tree == other.tree
        && one.command == other.command
        && one.result == other.result
        && match (one.at, other.at) {
            (Some(one), Some(other)) => one == other,
            (None, None) => one.seconds == other.seconds && one.host == other.host,
            _ => false,
        }
}

fn text(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::position::{Declared, Failure};
    use crate::stage_record::Stated;

    fn machine() -> Machine<'static> {
        Machine {
            chip: Some("Apple M4"),
            model: Some("Mac16,1"),
            cpus: Some(14),
            bytes: Some(34359738368),
        }
    }

    #[test]
    fn an_end_with_nothing_left_to_do_on_the_issue_leaves_the_account() {
        use End::*;
        for end in [Landed, Closed, Escalated, Ended, HumanClosed] {
            assert!(end.accounted(), "{end:?}");
        }
        // Work goes on after these, or the issue is gone.
        for end in [Parked, HandedOn, Forgotten] {
            assert!(!end.accounted(), "{end:?}");
        }
    }

    #[test]
    fn settling_a_landing_already_accounted_for_leaves_no_second_account() {
        assert_eq!(settled_end(true, true), None, "the landing turn was that end");
        assert_eq!(settled_end(true, false), Some(End::Landed), "a landing cut short");
        assert_eq!(settled_end(false, false), Some(End::HumanClosed));
        assert_eq!(settled_end(false, true), Some(End::HumanClosed), "the one comment is edited");
        assert!(settled_end(true, false).is_some_and(|end| end.accounted()));
    }

    #[test]
    fn an_account_is_read_by_its_marker() {
        assert!(left(["hello", "<!--convoy:account-->\n**afkconvoy**"]));
        assert!(!left(["hello", "<!--convoy:worker at=1-->"]));
    }

    fn hear(at: u64, body: &str) -> Heard<'_> {
        Heard { at: Some(at), body }
    }

    fn account(
        heard: &[Heard<'_>],
        gates: &[&str],
        claimed: bool,
        since: Option<u64>,
        end: u64,
    ) -> Option<String> {
        comment(&Told {
            issue: 7,
            repo: "acme/dev",
            tracker: "acme/tracker",
            version: "test",
            outcome: End::Landed,
            lanes: &[("./gate.sh", "whole")],
            heard,
            gates,
            claimed,
            held_since: since,
            end,
            machine: machine(),
            host: "mini",
        })
    }

    fn begun(
        at: u64,
        claim: &str,
        since: u64,
        harness: &str,
        model: Option<&str>,
        stage: Option<&str>,
    ) -> String {
        worker_comment(&Begun {
            at,
            claim,
            since: Some(since),
            harness,
            model,
            stage,
            role: None,
            consult: false,
        })
    }

    fn passed(stage: &str, claim: &str, harness: Harness, model: Option<&str>) -> String {
        StageRecord {
            stage: stage.to_string(),
            stated: Stated::Declared(Declared::Pass),
            by: Author::Worker {
                consult: false,
                claim: claim.to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness,
                model: model.map(str::to_string),
            },
            blocked_by: Default::default(),
            text: "done".to_string(),
        }
        .comment()
    }

    #[test]
    fn an_issue_that_was_never_claimed_has_no_account() {
        assert!(account(&[], &[], false, None, 100).is_none());
    }

    #[test]
    fn the_total_is_the_wall_clock_and_the_rows_are_not_added_into_it() {
        let start = begun(1200, "c1", 1000, "pi", Some("opus"), Some("build"));
        let record = passed("build", "c1", Harness::Pi, Some("opus"));
        let green = "convoy:gate tree=abc command=./gate.sh at=1800 seconds=42 host=mini issue=7";
        let body =
            account(&[hear(1200, &start), hear(2000, &record)], &[green], true, Some(1000), 2500)
                .unwrap();
        assert!(body.starts_with("<!--convoy:account"));
        assert_eq!(read(&body, 7).unwrap().start, Some(1000));
        assert_eq!(read(&body, 7).unwrap().end, Some(2500));
        assert!(body.contains("Wall clock: 1500s, from 1000 to 2500."));
        assert!(body.contains("800s from 1200 to 2000"));
        assert!(body.contains("green, 42s"));
        assert!(!body.contains("remainder"));
        assert!(!body.contains("842"));
        assert!(!body.contains("658"));
        assert!(StageRecord::read(&body).is_none());
        assert_eq!(record::markers(ACCOUNT, &body).len(), 1);
    }

    #[test]
    fn no_recorded_since_states_no_total() {
        let start = worker_comment(&Begun {
            at: 1200,
            claim: "c1",
            since: None,
            harness: "pi",
            model: None,
            stage: Some("build"),
            role: None,
            consult: false,
        });
        let body = account(&[hear(1200, &start)], &[], true, None, 2500).unwrap();
        assert!(body.contains("The start is unknown. No wall clock is stated."));
        assert!(!body.contains("Wall clock:"));
        assert!(!body.contains("start="));
        assert!(body.contains("the default model"));
        assert!(!body.contains("model `"));
    }

    #[test]
    fn a_later_end_does_not_move_the_start_forward() {
        let first = account(&[], &[], true, Some(1000), 1500).unwrap();
        let start = begun(4000, "c2", 4000, "pi", Some("opus"), Some("build"));
        let second =
            account(&[hear(1500, &first), hear(4000, &start)], &[], true, Some(4000), 5000)
                .unwrap();
        assert_eq!(read(&second, 7).unwrap().start, Some(1000));
        assert!(second.contains("Wall clock: 4000s, from 1000 to 5000."));
        assert_eq!(record::markers(ACCOUNT, &second).len(), 1);
    }

    #[test]
    fn an_unknown_start_stays_unknown() {
        let start = worker_comment(&Begun {
            at: 1200,
            claim: "c1",
            since: None,
            harness: "pi",
            model: None,
            stage: Some("build"),
            role: None,
            consult: false,
        });
        let first = account(&[hear(1200, &start)], &[], true, None, 1500).unwrap();
        assert!(!first.contains("start="), "{first}");
        let later = begun(4000, "c2", 4000, "codex", Some("opus"), Some("build"));
        let second = account(
            &[hear(1200, &start), hear(1500, &first), hear(4000, &later)],
            &[],
            true,
            Some(4000),
            5000,
        )
        .unwrap();
        assert!(!second.contains("start="), "{second}");
        assert!(second.contains("The start is unknown."));
        assert!(second.contains("No wall clock is stated."));
    }

    #[test]
    fn an_earlier_recorded_since_is_kept() {
        let first = account(&[], &[], true, Some(2000), 2500).unwrap();
        let start = begun(1000, "c0", 1000, "pi", None, Some("build"));
        let second =
            account(&[hear(2500, &first), hear(1000, &start)], &[], false, None, 3000).unwrap();
        assert_eq!(read(&second, 7).unwrap().start, Some(1000));
    }

    #[test]
    fn a_bound_issue_has_one_row_per_performance_and_a_failure_is_a_row() {
        let first = begun(1000, "c1", 1000, "pi", Some("opus"), Some("build"));
        let built = passed("build", "c1", Harness::Pi, Some("opus"));
        let again = begun(3000, "c2", 1000, "pi", None, Some("build"));
        let failed = StageRecord::failed("build", Failure::Silent, "gone").comment();
        let body = account(
            &[hear(1000, &first), hear(2000, &built), hear(3000, &again), hear(3500, &failed)],
            &[],
            true,
            Some(1000),
            4000,
        )
        .unwrap();
        let rows: Vec<_> = body.lines().filter(|line| line.starts_with("- harness")).collect();
        assert_eq!(rows.len(), 2, "{body}");
        assert!(rows[0].contains("verdict `pass`"));
        assert!(rows[0].contains("1000s from 1000 to 2000"));
        assert!(rows[1].contains("the default model"));
        assert!(rows[1].contains("verdict `failed-silent`"));
        assert!(rows[1].contains("500s from 3000 to 3500"));
        assert_eq!(read(&body, 7).unwrap().workers[1].verdict.as_deref(), Some("failed-silent"));
    }

    #[test]
    fn a_continuation_of_the_same_harness_is_not_another_row() {
        assert!(!own_row(false, Some("pi"), "pi"));
        assert!(own_row(false, Some("pi"), "codex"));
        assert!(!own_row(true, None, "pi"));
        assert!(own_row(false, None, "pi"));
        let noted = begun(1000, "c1", 900, "pi", None, Some("build"));
        assert!(already_noted([noted.as_str()], "c1", "pi"));
        assert!(!already_noted([noted.as_str()], "c1", "codex"));
        assert!(!already_noted([noted.as_str()], "c2", "pi"));
        let account = account(&[hear(1000, &noted)], &[], true, Some(900), 2000).unwrap();
        assert!(!already_noted([account.as_str()], "c1", "pi"));
    }

    #[test]
    fn an_unbound_worker_runs_to_the_end_and_a_later_harness_is_its_own_row() {
        let first = begun(1000, "c1", 900, "codex", Some("gpt"), None);
        let second = begun(1800, "c1", 900, "pi", None, None);
        let body = account(&[hear(1000, &first), hear(1800, &second)], &[], true, Some(900), 2500)
            .unwrap();
        let rows: Vec<_> = body.lines().filter(|line| line.starts_with("- harness")).collect();
        assert_eq!(rows.len(), 2, "{body}");
        assert!(rows[0].contains("1500s from 1000 to 2500"));
        assert!(rows[1].contains("700s from 1800 to 2500"));
        assert!(rows[1].contains("the default model"));
    }

    #[test]
    fn a_landing_start_is_not_a_row() {
        let landing = record::marker(
            WORKER,
            &[
                ("at", "1000"),
                ("claim", "c1"),
                ("harness", "pi"),
                ("stage", "land"),
                ("landing", "1"),
            ],
        );
        let body = account(&[hear(1000, &landing)], &[], true, Some(500), 2000).unwrap();
        assert!(body.contains("No worker was started."), "{body}");
    }

    #[test]
    fn gate_rows_are_this_issues_tree_and_a_red_one_is_kept() {
        let green = "convoy:gate tree=abc command=./gate.sh at=1800 seconds=42 host=mini issue=7";
        let red =
            "convoy:gate tree=def command=./gate.sh at=1900 seconds=17 host=other issue=7 red=1";
        let base = "convoy:gate tree=base command=./gate.sh at=1700 seconds=9 host=mini";
        let other = "convoy:gate tree=zzz command=./gate.sh at=1600 seconds=3 host=mini issue=8";
        let first = account(&[], &[green, red, base, other], true, Some(1000), 2500).unwrap();
        assert!(first.contains("green, 42s"));
        assert_eq!(read(&first, 7).unwrap().gates[0].tree.as_deref(), Some("abc"));
        assert!(first.contains("red, 17s"));
        assert!(first.contains("on host `other`"));
        assert!(!first.contains("tree `base`"), "{first}");
        assert!(!first.contains("tree `zzz`"), "{first}");
        assert_eq!(read(&first, 7).unwrap().gates[1].result.as_deref(), Some("red"));
        let again = account(&[hear(2500, &first)], &[], true, Some(1000), 2600).unwrap();
        assert!(!again.contains("green, 42s"), "{again}");
        assert!(again.contains("red, 17s"), "{again}");
        assert!(again.contains("on host `other`"));
    }

    #[test]
    fn a_gate_during_a_worker_does_not_shorten_that_row() {
        let start = begun(1000, "c1", 1000, "pi", Some("opus"), Some("build"));
        let record = passed("build", "c1", Harness::Pi, Some("opus"));
        let green = "convoy:gate tree=abc command=./gate.sh at=1500 seconds=42 host=mini issue=7";
        let body =
            account(&[hear(1000, &start), hear(2000, &record)], &[green], true, Some(1000), 2000)
                .unwrap();
        assert!(body.contains("1000s from 1000 to 2000"), "{body}");
        assert!(body.contains("green, 42s"));
    }

    #[test]
    fn the_machine_line_is_the_writing_host_and_names_nothing_it_did_not_report() {
        let body = account(&[], &[], true, Some(1), 2).unwrap();
        assert!(body.contains("Machine: Apple M4, 14 logical CPUs, 34359738368 bytes."));
        let bare = comment(&Told {
            issue: 7,
            repo: "acme/dev",
            tracker: "acme/tracker",
            version: "test",
            outcome: End::Landed,
            lanes: &[("./gate.sh", "whole")],
            heard: &[],
            gates: &[],
            claimed: true,
            held_since: Some(1),
            end: 2,
            machine: Machine { chip: Some(""), model: None, cpus: None, bytes: None },
            host: "mini",
        })
        .unwrap();
        assert!(bare.contains("Machine: chip unknown, logical CPUs unknown, memory unknown."));
    }

    #[test]
    fn a_command_with_html_comment_closers_and_fences_round_trips_as_data() {
        let first = account(&[], &[], true, Some(1), 2).unwrap();
        let mut row = read(&first, 7).unwrap();
        let command = "afkconvoy exclusive -- ./gate.sh\n```json\n</details><!-- --> & `";
        row.gates.push(Gate { command: Some(command.into()), ..Gate::default() });
        let body = render(&row);
        assert_eq!(read(&body, 7).unwrap(), row);
        let (_, block) = body.rsplit_once("```json\n").unwrap();
        let (json, _) = block.split_once("\n```").unwrap();
        for unsafe_text in ["</details>", "<!--", "-->", "```", "&"] {
            assert!(!json.contains(unsafe_text), "{json}");
        }
    }

    #[test]
    fn a_codex_worker_s_goal_round_trips_and_a_worker_of_another_harness_carries_none() {
        let first = account(&[], &[], true, Some(1), 2).unwrap();
        let mut row = read(&first, 7).unwrap();
        let goal = WorkerGoal {
            set: true,
            objective: Some("Read /b.md".into()),
            goal_turns: Some(3),
            goal_turns_after_verdict: Some(1),
            status: Some(CLEARED.into()),
            tokens_used: None,
            token_budget: None,
            session: Some("/kept/rollout.jsonl".into()),
        };
        row.workers.push(Worker {
            harness: Some("codex".into()),
            goal: Some(goal),
            ..Worker::default()
        });
        row.workers.push(Worker { harness: Some("claude-code".into()), ..Worker::default() });
        let body = render(&row);
        assert_eq!(read(&body, 7).unwrap(), row);
        assert!(
            body.contains(
                "goal `cleared`, 3 goal turns (1 after its verdict), goal tokens unknown"
            ),
            "{body}"
        );
        let (_, block) = body.rsplit_once("```json\n").unwrap();
        let (json, _) = block.split_once("\n```").unwrap();
        let json: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(json["workers"][0]["goal"]["status"], "cleared");
        assert_eq!(json["workers"][0]["goal"]["goal_turns_after_verdict"], 1);
        assert!(json["workers"][1].get("goal").is_none(), "{json}");
    }

    #[test]
    fn an_unreadable_prior_account_is_not_overwritten() {
        let old = "<!--convoy:account start=100 end=200-->";
        assert!(account(&[hear(200, old)], &[], true, Some(250), 300).is_none());
    }

    #[test]
    fn the_workers_role_survives_its_start_and_a_duration_before_start_is_unknown() {
        let start = worker_comment(&Begun {
            at: 100,
            claim: "c1",
            since: Some(100),
            harness: "codex",
            model: None,
            role: Some("developer"),
            stage: Some("build"),
            consult: false,
        });
        let body = account(&[hear(100, &start)], &[], true, Some(100), 90).unwrap();
        let row = read(&body, 7).unwrap();
        assert_eq!(row.workers[0].role.as_deref(), Some("developer"));
        assert_eq!(row.workers[0].seconds, None);
        assert_eq!(row.wall_seconds, None);
    }
}

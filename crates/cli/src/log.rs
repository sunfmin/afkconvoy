//! The log of a `run`: one line when it starts, one as each tick starts and
//! one as it ends, and one for each event that reached it — a wake heard
//! during a tick, a check for host sleep, the host waking, being told to stop
//! — in a file under this user's cache directory, for `doctor` to read back.
//! `run` prints each line too, as it is written (`Entry::said`). Every command
//! that changes fleet state writes a line of its own to it as it acts
//! (`crate::acting`), whether a `run` is there or not.
//!
//! It is a log, not state: nothing reads it to decide what the fleet does
//! (fleet state lives on GitHub and the remote), and a host with none loses
//! only what `doctor` could have said of the ticks before.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use afkconvoy_core::gate::key;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::copies::cache;
use crate::git::Repo;

/// How far back a log keeps every line, in seconds: a day.
const KEPT_FOR: u64 = 86_400;
/// The fewest lines a log is cut back to, however old they are.
const KEPT_LEAST: usize = 200;
/// The most lines a log holds, however new they are.
const KEPT_MOST: usize = 5000;

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// When it was written, in epoch seconds: for a tick, when it ended.
    pub at: u64,
    /// The fleet instance that wrote it, and its host.
    pub instance: String,
    pub host: String,
    #[serde(flatten)]
    pub what: What,
}

/// What a line says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// A `run` started, the worker launch command of its default harness
    /// beginning with this word: the word a login shell is asked about, and
    /// never what follows it. `harness_commands` is the same word of each
    /// other harness in the host kit, by the harness's key.
    Start {
        worker_command: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        harness_commands: BTreeMap<String, String>,
    },
    /// A tick started, for `cause`, before it did anything. `by` is what the
    /// wake that started it named — a command and what it acted on — where
    /// one did.
    TickStarted {
        count: u64,
        cause: Cause,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// One tick ended.
    Tick(Tick),
    /// A wake was heard while tick `during` ran: the tick after it starts as
    /// it ends. Only the first wake of a tick is a line; `by` is what it named.
    WokenDuring {
        during: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// A command that changes fleet state acted (`crate::acting`): `command`
    /// on `on` — `#46`, `run sweep`, `from <instance>` — run `by` a worker or
    /// the operator, coming to `outcome`, or failing with it. Written by that
    /// command, not by the `run`: `instance` is the fleet instance it acted
    /// as, or empty.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<String>,
        by: String,
        outcome: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        failed: bool,
    },
    /// A check for host sleep ended: it put the host to sleep, or what kept
    /// the host awake, each in words.
    HostSleep {
        slept: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        awake: Vec<String>,
    },
    /// The host woke from a sleep, noticed as a rest of this `run` took
    /// `asleep` seconds past what it was to.
    HostWoke { asleep: u64 },
    /// The `run` was told to stop, and said `line` of its heartbeat as it went.
    Stop { line: String },
    /// A worker turned hot (`crate::hot`): `worker` is `#7`, or its worktree's
    /// name; `why` and what it `held` as it did, in words.
    Hot { worker: String, why: String, held: String },
    /// A worker hot `since` then stopped being hot: under the limits for five
    /// minutes, or `gone` with its worktree. `held` is the most it took.
    Cooled {
        worker: String,
        since: u64,
        held: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        gone: bool,
    },
    /// The inspection of a hot worker started (`crate::inspection`), `by`
    /// the role it was started in.
    Inspecting { worker: String, by: String },
    /// The inspection of a hot worker ended, or none was started: `found` is
    /// what it found (`wrong`, `heavy`, `not-its`), `nothing` in time, or
    /// `none` where nobody inspects it; `said` is what it said, or why.
    Inspected { worker: String, found: String, said: String },
    /// Admission held back for `reason` past its span, and the host's
    /// inspection started (`crate::inspection`), `by` the role it was
    /// started in; `reading` is what admission read.
    HostInspecting { reason: String, by: String, reading: String },
    /// The host's inspection ended, or none was started: `found` is what it
    /// found (`reclaimable`, `fleet`, `not-its`), `nothing` in time, `failed`
    /// where it could not be started, or `covered` where another fleet of the
    /// host inspected it in the episode; `said` is what it said, or why.
    HostInspected { reason: String, found: String, said: String },
}

/// Why a tick started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    /// Its sleep ran out.
    Timer,
    /// A wake ended its sleep, or was heard during the tick before.
    Woken,
    /// It is the `run`'s first.
    Start,
    /// The tick before left a worker to start, or could not reach GitHub: it
    /// runs with no sleep.
    FollowUp,
}

impl Cause {
    pub fn said(self) -> &'static str {
        match self {
            Cause::Timer => "timer",
            Cause::Woken => "woken",
            Cause::Start => "start",
            Cause::FollowUp => "follow-up",
        }
    }
}

impl Entry {
    /// The line `run` prints of it on stdout, as it is written.
    pub fn said(&self) -> String {
        match &self.what {
            What::Start { worker_command, .. } => format!(
                "start: run as {} on {}, its workers started with `{worker_command}`",
                self.instance, self.host
            ),
            What::TickStarted { count, cause, by: Some(by) } => {
                format!("tick {count}: started ({} by {by})", cause.said())
            }
            What::TickStarted { count, cause, by: None } => {
                format!("tick {count}: started ({})", cause.said())
            }
            What::Tick(Tick { count, sleep: 0, line, .. }) => {
                format!("tick {count}: {line} · next now")
            }
            What::Tick(Tick { count, sleep, line, .. }) => {
                format!("tick {count}: {line} · next in {sleep}s")
            }
            What::WokenDuring { during, by } => {
                format!("{} during tick {during}: tick {} follows", woken(by), during + 1)
            }
            What::Command { command, on, by, outcome, failed } => {
                let on = on.as_ref().map(|on| format!(" {on}")).unwrap_or_default();
                let failed = if *failed { "FAILED: " } else { "" };
                format!("{command}{on} ({by}): {failed}{outcome}")
            }
            What::HostSleep { slept: true, .. } => "host sleep: every fleet on this host has \
                been idle long enough, and ticked once more: put the host to sleep"
                .to_string(),
            What::HostSleep { slept: false, awake } => {
                format!("host sleep: kept awake: {}", awake.join("; "))
            }
            What::HostWoke { asleep } => format!("host woke: after {asleep}s asleep"),
            What::Stop { line } => format!("stop: {line}"),
            What::Hot { worker, why, held } => {
                format!("hot: {worker} is hot: {why} · it holds {held}")
            }
            What::Cooled { worker, since, held, gone } => {
                let why = if *gone {
                    "its worktree is gone".to_string()
                } else {
                    "under the limits for five minutes".to_string()
                };
                format!(
                    "hot: {worker} is no longer hot: {why} · hot for {}s, it held at most {held}",
                    self.at.saturating_sub(*since)
                )
            }
            What::Inspecting { worker, by } => {
                format!("inspection: {worker} is inspected by {by}")
            }
            What::Inspected { worker, found, said } => {
                format!("inspection: {worker}: {found} · {said}")
            }
            What::HostInspecting { reason, by, reading } => format!(
                "host inspection: admission held back for {reason} past its span ({reading}): \
                 the host is inspected by {by}"
            ),
            What::HostInspected { reason, found, said } => {
                format!("host inspection: held back for {reason}: {found} · {said}")
            }
        }
    }

    /// What reached the `run` from outside it, as `doctor` lists it: the
    /// event's name and its cause in words; `None` of what the `run` did by
    /// itself — a tick on its timer, or after the one before, and its end.
    pub fn event(&self) -> Option<(&'static str, String)> {
        match &self.what {
            What::Start { worker_command, .. } => Some((
                "start",
                format!("afkconvoy run started, its workers with `{worker_command}`"),
            )),
            What::TickStarted { count, cause: Cause::Woken, by } => {
                Some(("wake", format!("{}: tick {count} started", woken(by))))
            }
            What::WokenDuring { during, by } => Some((
                "wake",
                format!("{} during tick {during}: tick {} followed", woken(by), during + 1),
            )),
            What::HostSleep { slept: true, .. } => {
                Some(("host_sleep", "every fleet on this host was idle".to_string()))
            }
            What::HostWoke { asleep } => Some(("host_wake", format!("after {asleep}s asleep"))),
            What::Stop { line } => {
                Some(("stop", format!("told to stop (SIGTERM or SIGINT): {line}")))
            }
            What::TickStarted { .. }
            | What::Tick(_)
            | What::HostSleep { slept: false, .. }
            | What::Command { .. }
            | What::Hot { .. }
            | What::Cooled { .. }
            | What::Inspecting { .. }
            | What::Inspected { .. }
            | What::HostInspecting { .. }
            | What::HostInspected { .. } => None,
        }
    }

    /// Whether a `run` wrote it, not a command that acted beside it: only
    /// such a line says which instance the `run` is.
    pub fn of_run(&self) -> bool {
        !matches!(self.what, What::Command { .. })
    }
}

/// A wake, in words: with what it named, where it named something.
fn woken(by: &Option<String>) -> String {
    match by {
        Some(by) => format!("woken by {by}"),
        None => "woken".to_string(),
    }
}

/// One tick, as its `run` saw it end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    /// Which tick of its `run` it was, from 1.
    pub count: u64,
    /// The seconds slept after it.
    pub sleep: u64,
    /// What `run` printed of it.
    pub line: String,
    /// The claims its pass could hold; `None` of a tick that failed whole.
    pub bound: Option<usize>,
    /// What `afkconvoy tick` answers for it; `None` of a tick that failed whole.
    pub answer: Option<Value>,
    /// Why it failed whole: it could not even rebuild.
    pub error: Option<String>,
}

/// One step that failed in a tick, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    /// What the step does (`dispatch`, `settle`, …); `tick` of a tick that
    /// failed whole.
    pub step: String,
    pub issue: Option<u64>,
    pub patrol: Option<String>,
    pub error: String,
}

impl Failed {
    /// Whether `other` is the same step on the same thing, whatever it said.
    pub fn same_step(&self, other: &Failed) -> bool {
        (&self.step, self.issue, &self.patrol) == (&other.step, other.issue, &other.patrol)
    }

    /// The step as a human names it: `dispatch #3`, `patrol triage`, `tick`.
    pub fn named(&self) -> String {
        let issue = self.issue.map(|issue| format!(" #{issue}")).unwrap_or_default();
        let patrol = self.patrol.as_ref().map(|patrol| format!(" {patrol}")).unwrap_or_default();
        format!("{}{issue}{patrol}", self.step)
    }
}

/// `failed` with every failure of one cause told once: the first of each
/// `error`, in the order they failed, with how many failed of it.
pub fn by_cause<T>(failed: &[T], error: impl Fn(&T) -> &str) -> Vec<(&T, usize)> {
    let mut causes: Vec<(&T, usize)> = vec![];
    for failure in failed {
        match causes.iter_mut().find(|(first, _)| error(first) == error(failure)) {
            Some((_, count)) => *count += 1,
            None => causes.push((failure, 1)),
        }
    }
    causes
}

impl Tick {
    /// The steps of this tick that failed.
    pub fn failed(&self) -> Vec<Failed> {
        if let Some(error) = &self.error {
            let step = "tick".to_string();
            return vec![Failed { step, issue: None, patrol: None, error: error.clone() }];
        }
        let errors = self.answer.as_ref().and_then(|answer| answer["errors"].as_array());
        let failed = |row: &Value| Failed {
            step: row["do"].as_str().unwrap_or_default().to_string(),
            issue: row["issue"].as_u64(),
            patrol: row["patrol"].as_str().map(str::to_string),
            error: row["error"].as_str().unwrap_or_default().to_string(),
        };
        errors.into_iter().flatten().map(failed).collect()
    }

    /// Whether this tick started a worker on an issue.
    pub fn dispatched(&self) -> bool {
        let did = self.answer.as_ref().and_then(|answer| answer["did"].as_array());
        let started = |step: &Value| step["do"] == "dispatch" && step["outcome"] == "done";
        did.into_iter().flatten().any(started)
    }

    /// Whether admission held back the workers this tick would have started:
    /// its host was busy.
    pub fn held_back(&self) -> bool {
        self.answer.as_ref().is_some_and(|answer| answer["held_back_by"].is_array())
    }

    /// The claims held and the frontier left as it ended.
    pub fn stood(&self) -> Option<(u64, u64)> {
        let answer = self.answer.as_ref()?;
        answer["held"].as_u64().zip(answer["frontier_remaining"].as_u64())
    }
}

/// The log of the `run` of one repo on this host.
pub struct Log {
    pub file: PathBuf,
    kept: Kept,
}

/// What a log keeps: every line of the last `for_secs` seconds, never fewer
/// than `least` lines nor more than `most`.
#[derive(Debug, Clone, Copy)]
struct Kept {
    for_secs: u64,
    least: usize,
    most: usize,
}

const KEPT: Kept = Kept { for_secs: KEPT_FOR, least: KEPT_LEAST, most: KEPT_MOST };

impl Kept {
    /// How many of the newest lines are kept of lines written at `ats`,
    /// oldest first, at `now`: those of the last day, at the least `least`
    /// and at the most `most` — never more than there are.
    fn of(self, ats: &[u64], now: u64) -> usize {
        let since = now.saturating_sub(self.for_secs);
        let recent = ats.iter().rev().take_while(|&&at| at >= since).count();
        recent.clamp(self.least, self.most).min(ats.len())
    }
}

impl Log {
    /// The log of the repo `repo` is a checkout of: one file for every
    /// worktree of it, named for its main checkout.
    pub fn of(repo: &Repo) -> Result<Log> {
        let main = repo.main_checkout()?;
        let name = main.file_name().map(|name| name.to_string_lossy().into_owned());
        let named =
            format!("{}-{:016x}.jsonl", name.unwrap_or_default(), key(&main.to_string_lossy()));
        Ok(Log::at(&cache()?.join("runs").join(named), KEPT))
    }

    fn at(file: &Path, kept: Kept) -> Log {
        Log { file: file.to_path_buf(), kept }
    }

    /// Appends `entry` as one line, written whole or not at all: what any
    /// command that acts does, while a `run` may be writing too. Lines
    /// appended at once are each whole, and none is lost to a `run` cutting
    /// the log back (`keep`): both are made holding the log's lock.
    pub fn append(&self, entry: &Entry) -> Result<()> {
        let _held = self.lock()?;
        self.write(entry)
    }

    /// The same, and then cuts the log back to what it keeps (`Kept`) once it
    /// holds more than its most, or `least` lines more than it keeps: a log
    /// is not written over at every line. A line already written is never
    /// touched again but to be dropped. Only a `run` cuts its log back.
    pub fn keep(&self, entry: &Entry) -> Result<()> {
        let _held = self.lock()?;
        self.write(entry)?;
        let text = std::fs::read_to_string(&self.file).with_context(|| self.unwritten())?;
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() <= self.kept.least {
            return Ok(());
        }
        /// Only when it was written: a line that cannot be read is as old as can be.
        #[derive(Deserialize)]
        struct At {
            at: u64,
        }
        let at = |line: &&str| serde_json::from_str::<At>(line).map_or(0, |at| at.at);
        let ats: Vec<u64> = lines.iter().map(at).collect();
        let kept = self.kept.of(&ats, entry.at);
        if lines.len() > self.kept.most || lines.len() >= kept + self.kept.least {
            // Whole or not at all: written beside it, then moved over it.
            let newest = lines[lines.len() - kept..].join("\n") + "\n";
            let beside = self.file.with_extension("trimming");
            std::fs::write(&beside, newest).with_context(|| self.unwritten())?;
            #[cfg(test)]
            tests::CUTTING.with(|cutting| cutting.get().map(|cutting| cutting()));
            std::fs::rename(&beside, &self.file).with_context(|| self.unwritten())?;
        }
        Ok(())
    }

    fn unwritten(&self) -> String {
        format!("cannot write the log {}", self.file.display())
    }

    /// Takes the log's lock, beside it: held until what it answers is
    /// dropped, or the process ends. Not the log itself, which a cut moves
    /// over: one appending to the file it opened before a cut would write to
    /// one that is gone.
    fn lock(&self) -> Result<std::fs::File> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).with_context(|| self.unwritten())?;
        }
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.file.with_extension("lock"))
            .with_context(|| self.unwritten())?;
        lock.lock().with_context(|| self.unwritten())?;
        Ok(lock)
    }

    /// Writes `entry` as one line, in one write to the end of the log.
    fn write(&self, entry: &Entry) -> Result<()> {
        let line = format!("{}\n", serde_json::to_string(entry)?);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.file)
            .with_context(|| self.unwritten())?;
        file.write_all(line.as_bytes()).with_context(|| self.unwritten())
    }

    /// Every line of the log, oldest first; `None` where there is no log. A
    /// line that cannot be read — one a newer afkconvoy wrote — is passed over.
    pub fn read(&self) -> Option<Vec<Entry>> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        Some(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    thread_local! {
        /// What a cut on this thread does between writing what it keeps and
        /// moving that over the log: a test holds it open.
        pub(super) static CUTTING: Cell<Option<fn()>> = const { Cell::new(None) };
    }

    fn tick(count: u64) -> Entry {
        let tick = Tick {
            count,
            sleep: 90,
            line: "nothing to do".into(),
            bound: Some(2),
            answer: Some(json!({"did": [], "errors": [], "held": 0, "frontier_remaining": 0})),
            error: None,
        };
        Entry {
            at: 1000 + count,
            instance: "alpha".into(),
            host: "h1".into(),
            what: What::Tick(tick),
        }
    }

    fn counts(log: &Log) -> Vec<u64> {
        let count = |entry: Entry| match entry.what {
            What::Tick(tick) => tick.count,
            _ => 0,
        };
        log.read().unwrap().into_iter().map(count).collect()
    }

    #[test]
    fn a_line_is_read_back_as_it_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::at(&dir.path().join("runs/widgets.jsonl"), SMALL);
        assert_eq!(log.read(), None, "no log yet");
        let start = Entry {
            at: 1000,
            instance: "alpha".into(),
            host: "h1".into(),
            what: What::Start {
                worker_command: "claude".into(),
                harness_commands: BTreeMap::new(),
            },
        };
        log.append(&start).unwrap();
        log.append(&tick(1)).unwrap();
        assert_eq!(log.read().unwrap(), [start, tick(1)]);
        let text = std::fs::read_to_string(&log.file).unwrap();
        assert_eq!(text.lines().count(), 2, "one line each: {text}");
    }

    /// A day of lines, at the least 3 and at the most 10.
    const SMALL: Kept = Kept { for_secs: 86_400, least: 3, most: 10 };

    /// A tick that ended at `at`.
    fn tick_at(count: u64, at: u64) -> Entry {
        Entry { at, ..tick(count) }
    }

    #[test]
    fn a_log_keeps_every_line_of_the_last_day_and_no_fewer_lines_than_its_least() {
        let kept = SMALL;
        let day = 86_400;
        // every line of the last day, the newest first
        assert_eq!(kept.of(&[0, 9, day, day + 5, day + 9], day + 10), 3, "old lines fall off");
        assert_eq!(kept.of(&[10, 20, 30, 40, 50], day + 10), 5, "a day back, to the second");
        // never fewer than its least, however old
        assert_eq!(kept.of(&[1, 2, 3, 4, 5], 10 * day), 3);
        assert_eq!(kept.of(&[1, 2], 10 * day), 2, "nor more than there are");
        // never more than its most, however new
        let new: Vec<u64> = (0..25).map(|at| day + at).collect();
        assert_eq!(kept.of(&new, day + 30), 10);
        // the bounds a `run` keeps its log at
        assert_eq!((KEPT.for_secs, KEPT.least, KEPT.most), (86_400, 200, 5000));
    }

    #[test]
    fn the_log_is_cut_back_to_its_newest_lines_but_never_past_a_day_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::at(&dir.path().join("widgets.jsonl"), SMALL);
        // old lines: cut back to its least once it holds that many more
        for count in 1..=5 {
            log.keep(&tick_at(count, count)).unwrap();
        }
        assert_eq!(counts(&log), [1, 2, 3, 4, 5]);
        log.keep(&tick_at(6, 6 + 86_400 * 2)).unwrap();
        assert_eq!(counts(&log), [4, 5, 6], "the least kept, the newest last");
        // lines of the last day: none dropped, up to its most
        let now = 86_400 * 3;
        for count in 7..=13 {
            log.keep(&tick_at(count, now + count)).unwrap();
        }
        assert_eq!(counts(&log), [7, 8, 9, 10, 11, 12, 13], "a day of lines kept whole");
        for count in 14..=60 {
            log.keep(&tick_at(count, now + count)).unwrap();
            assert!(counts(&log).len() <= 10, "never past its most");
            assert_eq!(counts(&log).last(), Some(&count), "the newest line is always kept");
        }
    }

    /// A line a command appends: `n`, written at `at`.
    fn acted(n: u64, at: u64) -> Entry {
        let what = What::Command {
            command: "park".into(),
            on: Some(format!("#{n}")),
            by: "operator".into(),
            outcome: "blocked by #99".into(),
            failed: false,
        };
        Entry { at, instance: String::new(), host: "h1".into(), what }
    }

    #[test]
    fn lines_appended_at_once_while_the_run_cuts_the_log_back_are_each_kept_whole() {
        let dir = tempfile::tempdir().unwrap();
        let kept = Kept { for_secs: 86_400, least: 5, most: 100 };
        let log = Log::at(&dir.path().join("widgets.jsonl"), kept);
        for count in 1..=104 {
            log.append(&tick_at(count, 1000)).unwrap();
        }
        // The run's cut takes a while between reading the log and moving
        // what it keeps over it, while four commands append, one line at a
        // time, all along.
        let (writers, lines) = (4, 50);
        std::thread::scope(|threads| {
            let log = &log;
            threads.spawn(move || {
                CUTTING.with(|cutting| {
                    cutting.set(Some(|| std::thread::sleep(Duration::from_millis(150))))
                });
                std::thread::sleep(Duration::from_millis(20));
                log.keep(&tick_at(105, 1000)).unwrap();
            });
            for writer in 1..=writers {
                threads.spawn(move || {
                    for n in 0..lines {
                        log.append(&acted(writer * 1000 + n, 1000)).unwrap();
                        std::thread::sleep(Duration::from_millis(5));
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&log.file).unwrap();
        let entries: Vec<Entry> =
            text.lines().map(|line| serde_json::from_str(line).expect(line)).collect();
        // A cut keeps the newest lines: what is left of each command's is the
        // end of what it wrote, none missing in between.
        let mut ends: Vec<Vec<u64>> = vec![vec![]; writers as usize + 1];
        for entry in &entries {
            match &entry.what {
                What::Command { on: Some(on), .. } => {
                    let n: u64 = on.trim_start_matches('#').parse().unwrap();
                    ends[(n / 1000) as usize].push(n % 1000);
                }
                What::Tick(tick) => ends[0].push(tick.count),
                other => panic!("{other:?}"),
            }
        }
        for (writer, end) in ends.iter().enumerate().skip(1) {
            let first = end.first().copied().unwrap_or(lines);
            assert_eq!(*end, (first..lines).collect::<Vec<_>>(), "command {writer}");
        }
        let cut = ends[0].first().copied().unwrap_or_default();
        assert!(cut > 1, "the run cut the log back: {:?}", ends[0]);
        assert_eq!(*ends[0], (cut..=105).collect::<Vec<_>>());
    }

    #[test]
    fn a_command_says_what_it_did_on_what_and_who_ran_it() {
        assert_eq!(acted(46, 1).said(), "park #46 (operator): blocked by #99");
        let failed = What::Command {
            command: "verdict".into(),
            on: Some("#121".into()),
            by: "worker".into(),
            outcome: "no claim".into(),
            failed: true,
        };
        let failed = Entry { what: failed, ..acted(1, 1) };
        assert_eq!(failed.said(), "verdict #121 (worker): FAILED: no claim");
        assert!(!failed.of_run());
        assert!(failed.event().is_none());
        let woken = |by: Option<&str>| Entry {
            what: What::TickStarted { count: 5, cause: Cause::Woken, by: by.map(str::to_string) },
            ..tick(1)
        };
        assert_eq!(woken(Some("park #46")).said(), "tick 5: started (woken by park #46)");
        assert_eq!(woken(None).said(), "tick 5: started (woken)");
        let during =
            Entry { what: What::WokenDuring { during: 4, by: Some("land #7".into()) }, ..tick(1) };
        assert_eq!(during.said(), "woken by land #7 during tick 4: tick 5 follows");
        assert!(during.of_run());
    }

    #[test]
    fn a_line_that_cannot_be_read_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::at(&dir.path().join("widgets.jsonl"), SMALL);
        log.keep(&tick(1)).unwrap();
        let mut text = std::fs::read_to_string(&log.file).unwrap();
        text.push_str("{\"at\": 5, \"cut sho\n");
        std::fs::write(&log.file, text).unwrap();
        log.keep(&tick(2)).unwrap();
        assert_eq!(counts(&log), [1, 2]);
    }

    #[test]
    fn one_cause_is_told_once_with_the_steps_it_stopped() {
        let causes = by_cause(&["reset", "no", "reset", "reset"], |error| error);
        assert_eq!(causes, [(&"reset", 3), (&"no", 1)]);
    }

    #[test]
    fn a_tick_says_which_of_its_steps_failed() {
        let mut failed = tick(1);
        let What::Tick(tick) = &mut failed.what else { unreachable!() };
        tick.answer = Some(json!({"did": [{"do": "dispatch", "issue": 5, "outcome": "done"}],
            "errors": [{"do": "dispatch", "issue": 3, "error": "orca worktree create failed"},
                {"do": "patrol", "issue": null, "patrol": "triage", "error": "no"}],
            "held": 2, "frontier_remaining": 1}));
        let steps: Vec<String> = tick.failed().iter().map(Failed::named).collect();
        assert_eq!(steps, ["dispatch #3", "patrol triage"]);
        let failed = tick.failed();
        let causes = by_cause(&failed, |failed| &failed.error);
        assert_eq!(causes.len(), 2, "two causes: {causes:?}");
        assert!(tick.dispatched());
        assert_eq!(tick.stood(), Some((2, 1)));
        // a tick that could not rebuild failed whole
        tick.answer = None;
        tick.error = Some("gh pr list failed".into());
        assert_eq!(tick.failed()[0].named(), "tick");
        assert!(!tick.dispatched());
    }
}

//! What the log of a `run` says of a host (`crate::log`): the fleet instance
//! it ran last, with the worker launch commands it started with and its
//! ticks; how long its last tick took and the one under way; the workers it
//! found hot; its break; and the events that reached it. `doctor`, the panel,
//! `checkouts` and `stop --release` each read it here, from the lines they
//! read once.

use serde::Serialize;

use crate::log::{Entry, Tick, What};

/// How many of the events that reached a `run` are answered, newest first.
pub(crate) const EVENTS_SHOWN: usize = 20;

/// The worker launch commands a `run` said it started with: the first word
/// of each, retained only as log evidence, never used to select commands.
pub(crate) type StartedWith = Vec<String>;

/// A worker that is hot, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotRead {
    /// `#7`, or the name of its worktree.
    pub worker: String,
    /// When it turned hot, why, and what it held as it did, in words.
    pub since: u64,
    pub why: String,
    pub held: String,
}

/// A fleet instance that is broken, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrokenRead {
    /// Why it broke, as it last did.
    pub why: String,
    /// When it broke: a start of it again that failed since is no end of it.
    pub since: u64,
    /// When the host runner starts it again; `None`: no host runner runs on
    /// this host to.
    pub next: Option<u64>,
    /// The fleet instance that broke; empty where it broke before it was any.
    pub instance: String,
}

/// One event that reached a `run` from outside it: a wake, its start or
/// stop, the host waking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub at: u64,
    /// `wake`, `start`, `stop` or `host_wake`, among others (`Entry::event`).
    pub event: &'static str,
    /// What caused it, in words.
    pub cause: String,
    pub instance: String,
}

/// What the log says of the `run` of `host`: its fleet instance, the worker
/// launch commands it started with, and that instance's ticks.
pub(crate) fn run(
    entries: &[Entry],
    host: &str,
) -> (Option<String>, Option<StartedWith>, Vec<(u64, Tick)>) {
    // A command's line names the instance it acted as, not the `run`'s.
    let Some(newest) = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())
    else {
        return (None, None, vec![]);
    };
    let own = || entries.iter().filter(|entry| entry.instance == newest.instance);
    let command = own().rev().find_map(|entry| match &entry.what {
        What::Start { worker_command, harness_commands } => {
            let named = harness_commands.iter().map(|(harness, word)| format!("{harness}={word}"));
            Some(std::iter::once(worker_command.clone()).chain(named).collect())
        }
        _ => None,
    });
    let ticks = own().filter_map(|entry| match &entry.what {
        What::Tick(tick) => Some((entry.at, tick.clone())),
        _ => None,
    });
    (Some(newest.instance.clone()), command, ticks.collect())
}

/// The seconds the last tick of the newest `run` of `host` took, from the
/// line it started with to the one it ended with, as the log has them.
pub(crate) fn last_took(entries: &[Entry], host: &str) -> Option<u64> {
    let newest = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())?;
    let mut own = entries.iter().rev().filter(|entry| entry.instance == newest.instance);
    let (ended, count) = own.find_map(|entry| match &entry.what {
        What::Tick(tick) => Some((entry.at, tick.count)),
        _ => None,
    })?;
    own.find_map(|entry| match &entry.what {
        What::TickStarted { count: started, .. } if *started == count => {
            Some(ended.saturating_sub(entry.at))
        }
        _ => None,
    })
}

/// The unfinished tick of the newest `run` of `host`. Other commands and
/// events do not end a tick, and another instance's ticks are no evidence.
pub(crate) fn tick_started(entries: &[Entry], host: &str) -> Option<u64> {
    let newest = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())?;
    let latest = entries.iter().rev().find(|entry| {
        entry.host == host
            && entry.instance == newest.instance
            && matches!(
                entry.what,
                What::TickStarted { .. }
                    | What::Tick(_)
                    | What::Start { .. }
                    | What::Stop { .. }
                    | What::Broken { .. }
            )
    })?;
    matches!(latest.what, What::TickStarted { .. }).then_some(latest.at)
}

/// The workers the newest `run` of `host` found hot and has not found
/// stopped being hot since, as the log has them, longest hot first.
pub(crate) fn hot(entries: &[Entry], host: &str) -> Vec<HotRead> {
    let Some(newest) = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())
    else {
        return vec![];
    };
    let mut hot: Vec<HotRead> = vec![];
    for entry in entries.iter().filter(|entry| entry.instance == newest.instance) {
        match &entry.what {
            What::Hot { worker, why, held } => {
                hot.retain(|read| &read.worker != worker);
                let (worker, why, held) = (worker.clone(), why.clone(), held.clone());
                hot.push(HotRead { worker, since: entry.at, why, held });
            }
            What::Cooled { worker, .. } => hot.retain(|read| &read.worker != worker),
            _ => {}
        }
    }
    hot
}

/// The fleet instance of `host` broken, as the log has it: its newest line
/// of a start, a break, a coming back or a stop is a break. `runs` says whether a
/// host runner runs on this host, which starts it again.
pub(crate) fn broken(entries: &[Entry], host: &str, runs: bool) -> Option<BrokenRead> {
    let of_host = entries.iter().rev().filter(|entry| entry.host == host);
    // A `stop:` line after it broke: `stop` took its checkout off the list.
    let mut starts = of_host.filter(|entry| {
        matches!(
            entry.what,
            What::Start { .. } | What::Broken { .. } | What::Back { .. } | What::Stop { .. }
        )
    });
    let newest = starts.next()?;
    let What::Broken { why, since, next } = &newest.what else { return None };
    let (why, since, next) = (why.clone(), *since, runs.then_some(*next));
    Some(BrokenRead { why, since, next, instance: newest.instance.clone() })
}

/// The events that reached the `run`s of `host` (`Entry::event`), as the log
/// has them: the newest `EVENTS_SHOWN`, newest first, whichever instance they
/// reached — a `run` stopped and started again is two.
pub(crate) fn events(entries: &[Entry], host: &str) -> Vec<Event> {
    let of_host = entries.iter().rev().filter(|entry| entry.host == host);
    let event = |entry: &Entry| {
        let (event, cause) = entry.event()?;
        Some(Event { at: entry.at, event, cause, instance: entry.instance.clone() })
    };
    of_host.filter_map(event).take(EVENTS_SHOWN).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::log::Cause;

    /// A tick that ended with nothing to do.
    fn tick(count: u64) -> Tick {
        let answer = json!({"did": [], "errors": [], "held": 2, "frontier_remaining": 0});
        Tick {
            count,
            sleep: 90,
            line: "nothing to do".into(),
            answer: Some(answer),
            error: None,
        }
    }

    #[test]
    fn the_events_that_reached_this_hosts_runs_are_listed_newest_first_with_their_causes() {
        let entry = |at: u64, instance: &str, host: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: host.into(),
            what,
        };
        let start = What::Start { worker_command: "claude".into(), harness_commands: [].into() };
        let started = |count, cause| What::TickStarted { count, cause, by: None };
        let entries = [
            entry(10, "old", "h1", start.clone()),
            entry(20, "old", "h1", What::Stop { line: "holding no claim".into() }),
            entry(30, "alpha", "h1", start.clone()),
            entry(30, "alpha", "h1", started(1, Cause::Start)),
            entry(40, "alpha", "h1", What::Tick(tick(1))),
            entry(50, "alpha", "h1", started(2, Cause::Timer)),
            entry(55, "alpha", "h1", What::WokenDuring { during: 2, by: None }),
            entry(60, "alpha", "h1", started(3, Cause::Woken)),
            entry(90, "alpha", "h1", What::HostWoke { asleep: 3600 }),
            entry(95, "beta", "h2", start),
        ];
        let events = super::events(&entries, "h1");
        let said: Vec<(u64, &str, &str, &str)> = events
            .iter()
            .map(|event| (event.at, event.event, event.cause.as_str(), event.instance.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (90, "host_wake", "after 3600s asleep", "alpha"),
                (60, "wake", "woken: tick 3 started", "alpha"),
                (55, "wake", "woken during tick 2: tick 3 followed", "alpha"),
                (30, "start", "afkconvoy run started, its workers with `claude`", "alpha"),
                (20, "stop", "told to stop (SIGTERM or SIGINT): holding no claim", "old"),
                (10, "start", "afkconvoy run started, its workers with `claude`", "old"),
            ]
        );
        // only the newest are listed
        let many: Vec<Entry> =
            (0..30).map(|at| entry(at, "alpha", "h1", started(at, Cause::Woken))).collect();
        let events = super::events(&many, "h1");
        assert_eq!(events.len(), EVENTS_SHOWN);
        assert_eq!(events[0].at, 29);
    }

    #[test]
    fn the_hot_workers_are_those_the_newest_run_found_hot_and_not_cooled_since() {
        let entry = |at: u64, instance: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: "h1".into(),
            what,
        };
        let hot = |worker: &str| What::Hot {
            worker: worker.into(),
            why: "past 4.0 cores for 120s".into(),
            held: "6.0 cores, 1.0 GiB resident".into(),
        };
        let cooled = |worker: &str| What::Cooled {
            worker: worker.into(),
            since: 0,
            held: "6.0 cores, 1.0 GiB resident".into(),
            gone: false,
        };
        let start = What::Start { worker_command: "claude".into(), harness_commands: [].into() };
        let entries = [
            entry(10, "old", start.clone()),
            entry(20, "old", hot("#1")),
            entry(30, "alpha", start),
            entry(40, "alpha", hot("#7")),
            entry(50, "alpha", hot("#8")),
            entry(60, "alpha", cooled("#7")),
            entry(70, "alpha", hot("#9")),
        ];
        let read = super::hot(&entries, "h1");
        let said: Vec<(&str, u64)> =
            read.iter().map(|hot| (hot.worker.as_str(), hot.since)).collect();
        // #1 was the old run's, and #7 cooled
        assert_eq!(said, [("#8", 50), ("#9", 70)]);
        assert!(super::hot(&entries, "h2").is_empty());
    }

    #[test]
    fn the_log_is_read_for_the_instance_this_host_ran_last() {
        let entry = |at: u64, instance: &str, host: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: host.into(),
            what,
        };
        let start = |command: &str| What::Start {
            worker_command: command.into(),
            harness_commands: [("pi".to_string(), "pi".to_string())].into(),
        };
        let ticked = |count| What::Tick(tick(count));
        let entries = [
            entry(10, "old", "h1", start("agent")),
            entry(20, "old", "h1", ticked(1)),
            entry(30, "alpha", "h1", start("claude --yolo")),
            entry(40, "alpha", "h1", ticked(1)),
            entry(50, "alpha", "h1", ticked(2)),
        ];
        let (instance, command, ticks) = run(&entries, "h1");
        assert_eq!(instance.as_deref(), Some("alpha"));
        // as the legacy start record represents them: the default harness's bare
        assert_eq!(command.unwrap(), ["claude --yolo", "pi=pi"]);
        let counts: Vec<(u64, u64)> = ticks.iter().map(|(at, tick)| (*at, tick.count)).collect();
        assert_eq!(counts, [(40, 1), (50, 2)], "its own ticks only");
        assert_eq!(run(&entries, "h2"), (None, None, vec![]));
    }
}

//! The disk floor tells the workers already at work (sunfmin/control#1112).
//! Admission holds new workers once the disk under the checkout is below its
//! floor, but the workers running keep writing: a pile of worktrees, each
//! with its builds, filled a host's disk. So while admission holds for the
//! disk, each worker of this fleet at work is told once to stop writing, and
//! once the disk is over its floor again, each one told is told to go on.

use afkconvoy_core::admission::Hold;
use afkconvoy_core::worker::WorkerState;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::measure::said;
use crate::orca::Handed;

/// The note, in a worktree's git dir, that its worker was told to stop
/// writing: when, in epoch seconds.
const TOLD: &str = "convoy-disk-floor";

/// What a worker is to be told, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Say {
    Stop { free: u64, floor: u64 },
    GoOn,
}

/// What a worker that was `told` and is `busy` is told while the disk is
/// `below` its floor — `(free, floor)` — or over it: only a worker at work
/// is told to stop, since one stopped writes nothing and a note would start
/// it, and only one told to stop is told to go on.
fn due(told: bool, busy: bool, below: Option<(u64, u64)>) -> Option<Say> {
    match below {
        Some((free, floor)) if !told && busy => Some(Say::Stop { free, floor }),
        None if told => Some(Say::GoOn),
        _ => None,
    }
}

fn line(say: &Say) -> String {
    match say {
        Say::Stop { free, floor } => format!(
            "The disk under this checkout has {} free, under its floor of {}: the host is \
             running out of disk. Stop writing now: run no build, test, gate or download, and \
             write no large file. Commit and push what you have, then end your turn; the fleet \
             tells you when the disk is free again. [disk floor]",
            said(*free),
            said(*floor)
        ),
        Say::GoOn => "The disk under this checkout is over its floor again: go on with your \
                      task where you stopped. [disk floor]"
            .to_string(),
    }
}

/// The disk hold among `held`, admission's holds this tick: `(free, floor)`.
pub(crate) fn below(held: &[Hold]) -> Option<(u64, u64)> {
    held.iter().find_map(|hold| match hold {
        Hold::Disk { free, floor } => Some((*free, *floor)),
        _ => None,
    })
}

/// The line typed at a worker and not yet given to its model, in its
/// worktree's git dir: a `Pending`, kept until its session holds the mark.
const PENDING: &str = "convoy-disk-floor-line";

/// The terminal that took the pending line (`Worktree::hand_update`): while
/// it holds, the line is not typed again.
const RECEIPT: &str = "convoy-disk-floor-receipt";

/// A line typed at a worker, or about to be, that its harness has not given
/// its model yet. It carries its `mark`, which its session shows once the
/// model has it; until then each tick looks once and goes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    /// Whether it tells the worker to stop: once taken, it is told.
    stop: bool,
    line: String,
    mark: String,
    /// When it was first typed, epoch seconds.
    at: u64,
}

impl Pending {
    fn new(say: &Say, now: u64) -> Self {
        let mark = format!("convoy-disk-floor-{now}");
        // The mark is in the line: the session holds the line as the user's.
        let line = format!("{} [disk floor: {mark}]", line(say));
        Pending { stop: matches!(say, Say::Stop { .. }), line, mark, at: now }
    }
}

/// Tells each worker of `issues`, this fleet's on `host`, what the disk under
/// the checkout calls for (`due`). Each telling is a tick step; one that
/// fails costs only itself and is tried again at the next tick. A line is
/// typed once and never waited on: the tick looks once whether the harness
/// gave it to its model, and a line still queued is looked at again at the
/// next tick (sunfmin/control#1230). One queued past the note bound is typed
/// again, under the same mark.
pub(crate) fn reconcile(fleet: &Fleet, host: &str, issues: &[u64], held: &[Hold]) -> Vec<Value> {
    let below = below(held);
    let mut steps = vec![];
    for &issue in issues {
        let made = (|| -> Result<Option<Value>> {
            let Ok((worktree, terminal)) = crate::tell::worker(fleet, host, issue)? else {
                return Ok(None);
            };
            let told = worktree.note(TOLD)?;
            let pending_file = worktree.note(PENDING)?;
            let receipt = worktree.note(RECEIPT)?;
            // A line typed and not taken yet is handed before any other:
            // whichever it says, the next tick's `due` sets right what it did.
            let pending = std::fs::read_to_string(&pending_file)
                .ok()
                .and_then(|text| serde_json::from_str::<Pending>(&text).ok());
            let pending = match pending {
                Some(pending) => pending,
                None => {
                    let is_told = told.exists();
                    // Reading a worker's state asks orca: only where it matters.
                    let busy = !is_told
                        && below.is_some()
                        && worktree.state(fleet.now)? == WorkerState::Busy;
                    let Some(say) = due(is_told, busy, below) else { return Ok(None) };
                    let pending = Pending::new(&say, fleet.now);
                    std::fs::write(&pending_file, serde_json::to_string(&pending)?)?;
                    pending
                }
            };
            let did = if pending.stop { "stop" } else { "go on" };
            let since = worktree.handed(None)?.unwrap_or(0);
            let handed =
                worktree.hand_update(&terminal, &pending.line, &pending.mark, since, &receipt)?;
            if handed == Handed::Queued {
                let bound = crate::orca::note_wait_seconds();
                let mut step = json!({"do": "disk_floor", "issue": issue, "outcome": "queued",
                    "since": pending.at, "comment": format!("the disk floor, {did}")});
                if fleet.now.saturating_sub(pending.at) >= bound {
                    // Not taken within the bound: typed again at the next
                    // tick, under the same mark, rather than waited on.
                    let _ = std::fs::remove_file(&receipt);
                    step["outcome"] = json!("failed");
                    step["error"] = json!(format!(
                        "the harness did not give its model the line queued since {} within \
                         {bound}s: it is typed again at the next tick",
                        pending.at
                    ));
                }
                return Ok(Some(step));
            }
            if pending.stop {
                std::fs::write(&told, format!("{}\n", fleet.now))?;
            } else if told.exists() {
                std::fs::remove_file(&told)?;
            }
            let _ = std::fs::remove_file(&receipt);
            std::fs::remove_file(&pending_file)?;
            Ok(Some(json!({"do": "disk_floor", "issue": issue, "outcome": "done",
                "comment": format!("the disk floor, {did}")})))
        })();
        match made {
            Ok(step) => steps.extend(step),
            Err(error) => steps.push(json!({"do": "disk_floor", "issue": issue,
                "outcome": "failed", "error": format!("{error:#}")})),
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn a_worker_at_work_is_told_once_to_stop_below_the_floor_and_to_go_on_over_it() {
        let below = Some((GIB, 4 * GIB));
        assert_eq!(due(false, true, below), Some(Say::Stop { free: GIB, floor: 4 * GIB }));
        assert_eq!(due(true, true, below), None, "told once");
        assert_eq!(due(false, false, below), None, "a stopped worker writes nothing");
        assert_eq!(due(true, false, None), Some(Say::GoOn));
        assert_eq!(due(false, true, None), None, "never told, nothing to take back");
        assert_eq!(
            below_of(&[
                Hold::Load { load: 3, limit: 2, busy: None },
                Hold::Disk { free: 1, floor: 2 }
            ]),
            Some((1, 2))
        );
        assert_eq!(below_of(&[Hold::Load { load: 3, limit: 2, busy: None }]), None);
        assert!(
            line(&due(false, true, below).unwrap())
                .contains("1.0 GiB free, under its floor of 4.0 GiB")
        );
    }

    fn below_of(held: &[Hold]) -> Option<(u64, u64)> {
        below(held)
    }
}

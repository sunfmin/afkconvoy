//! The disk floor tells the workers already at work (sunfmin/control#1112).
//! Admission holds new workers once the disk under the checkout is below its
//! floor, but the workers running keep writing: a pile of worktrees, each
//! with its builds, filled a host's disk. So while admission holds for the
//! disk, each worker of this fleet at work is told once to stop writing, and
//! once the disk is over its floor again, each one told is told to go on.

use afkconvoy_core::admission::Hold;
use afkconvoy_core::worker::WorkerState;
use anyhow::Result;
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::measure::said;
use crate::orca::Worktree;

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

/// Tells each worker of `issues`, this fleet's on `host`, what the disk under
/// the checkout calls for (`due`). Each telling is a tick step; one that
/// fails costs only itself and is tried again at the next tick.
pub(crate) fn reconcile(fleet: &Fleet, host: &str, issues: &[u64], held: &[Hold]) -> Vec<Value> {
    let below = below(held);
    let mut steps = vec![];
    for &issue in issues {
        let made = (|| -> Result<Option<Value>> {
            let Ok((worktree, terminal)) = crate::tell::worker(fleet, host, issue)? else {
                return Ok(None);
            };
            let note = worktree.note(TOLD)?;
            let told = note.exists();
            // Reading a worker's state asks orca: only where it matters.
            let busy = !told && below.is_some() && worktree.state(fleet.now)? == WorkerState::Busy;
            let say = due(told, busy, below);
            let Some(say) = say else { return Ok(None) };
            tell(&worktree, &terminal, &say, fleet.now)?;
            match say {
                Say::Stop { .. } => std::fs::write(&note, format!("{}\n", fleet.now))?,
                Say::GoOn => std::fs::remove_file(&note)?,
            }
            let did = if matches!(say, Say::Stop { .. }) { "stop" } else { "go on" };
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

fn tell(worktree: &Worktree, terminal: &str, say: &Say, now: u64) -> Result<()> {
    let mark = format!("convoy-disk-floor-{now}");
    let since = worktree.handed(None)?.unwrap_or(0);
    worktree.hand_note(terminal, &line(say), &mark, since)?;
    Ok(())
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

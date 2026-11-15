//! A stopped candidate's processes that take long to exit. Its turn waits
//! for them with no bound (`candidates::confirmed`); past
//! `candidates::LONG_AFTER` it records here what is left, each pid with its
//! state and command, for as long as it waits, and `doctor` says each wait
//! whose turn is alive as a finding of the host for its watch
//! (sunfmin/control#1096). The record goes once they are gone.
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use afkconvoy_core::candidate::Process;

use crate::doctor::{Finding, Kind, finding};

/// One process a stopped candidate leaves, as `ps` said it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Left {
    pub pid: u32,
    /// Its state: `U` or `D` is uninterruptible I/O.
    pub state: String,
    pub command: String,
}

/// A wait for a stopped candidate's processes that has run long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Waiting {
    pub candidate: String,
    /// The lane it keeps occupied.
    pub lane: String,
    /// Since when, in Unix seconds.
    pub since: u64,
    /// The process that waits: the record stands while it does.
    pub waiter: Process,
    /// Whether the candidate's process scope is still held, by processes
    /// named or not.
    pub held: bool,
    /// Its process scope, the file its processes hold.
    pub scope: String,
    pub left: Vec<Left>,
}

/// The lane candidate `id` keeps while its processes exit.
pub(crate) fn lane(id: &str) -> String {
    format!("the worker lane of candidate {id}")
}

fn dir() -> Result<PathBuf> {
    Ok(crate::copies::cache()?.join("candidate-exits"))
}

fn file(id: &str) -> Result<PathBuf> {
    Ok(dir()?.join(format!("{id}.json")))
}

/// Records that candidate `id`'s processes `pids`, and its `scope` where
/// `held`, are still there after `waited`.
pub(crate) fn waiting(
    id: &str,
    scope: &std::path::Path,
    pids: &[u32],
    held: bool,
    waited: Duration,
) -> Result<()> {
    let waiter = crate::candidates::identity(std::process::id())?
        .ok_or_else(|| anyhow::anyhow!("cannot identify the waiting process"))?;
    let record = Waiting {
        candidate: id.to_string(),
        lane: lane(id),
        since: crate::clock().saturating_sub(waited.as_secs()),
        waiter,
        held,
        scope: scope.to_string_lossy().into_owned(),
        left: listed(pids),
    };
    let path = file(id)?;
    std::fs::create_dir_all(dir()?)?;
    let written = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&written, serde_json::to_vec(&record)?)?;
    std::fs::rename(&written, &path)?;
    Ok(())
}

/// Candidate `id`'s processes are gone: its record, if any, goes. Soft: one
/// that cannot be removed names a waiter that is no more, which `doctor`
/// passes over.
pub(crate) fn gone(id: &str) {
    if let Ok(path) = file(id) {
        let _ = std::fs::remove_file(path);
    }
}

/// `pids` as `ps` says them; one it no longer lists is left out.
fn listed(pids: &[u32]) -> Vec<Left> {
    if pids.is_empty() {
        return vec![];
    }
    let asked: Vec<String> = pids.iter().map(u32::to_string).collect();
    let Ok(out) = crate::process::output(Command::new("ps").args([
        "-o",
        "pid=,stat=,command=",
        "-p",
        &asked.join(","),
    ])) else {
        return vec![];
    };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(parsed).collect()
}

fn parsed(line: &str) -> Option<Left> {
    let mut words = line.split_whitespace();
    let pid = words.next()?.parse().ok()?;
    let state = words.next()?.to_string();
    Some(Left { pid, state, command: words.collect::<Vec<_>>().join(" ") })
}

/// Each long wait whose waiter is alive, as a finding of the host. Soft: a
/// record that cannot be read is passed over.
pub(crate) fn findings(now: u64) -> Vec<Finding> {
    let Ok(entries) = dir().and_then(|dir| Ok(std::fs::read_dir(dir)?)) else { return vec![] };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            serde_json::from_slice::<Waiting>(&std::fs::read(entry.path()).ok()?).ok()
        })
        .filter(|record| {
            crate::candidates::identity(record.waiter.pid)
                .is_ok_and(|current| current.as_ref() == Some(&record.waiter))
        })
        .map(|record| said(&record, now))
        .collect()
}

/// The finding `record` is at `now`.
pub(crate) fn said(record: &Waiting, now: u64) -> Finding {
    let minutes = now.saturating_sub(record.since) / 60;
    let left: Vec<String> = record
        .left
        .iter()
        .map(|left| format!("pid {} ({}) `{}`", left.pid, left.state, left.command))
        .collect();
    let mut evidence = if left.is_empty() {
        "no process it can name is left".to_string()
    } else {
        left.join("; ")
    };
    if record.held {
        evidence.push_str("; its process scope is still held");
    }
    evidence.push_str(&format!(
        "; pid {} keeps waiting, killing them hard each second",
        record.waiter.pid
    ));
    let pids: Vec<String> = record.left.iter().map(|left| left.pid.to_string()).collect();
    let fix = if pids.is_empty() {
        format!("lsof {}", record.scope)
    } else {
        format!("ps -o pid,stat,wchan,user,command -p {}", pids.join(","))
    };
    finding(
        Kind::ExitWaiting,
        json!(["exit-waiting", record.candidate]),
        format!(
            "candidate {} {} {minutes} min, keeping {} occupied",
            record.candidate,
            Kind::ExitWaiting.symptom(),
            record.lane
        ),
        evidence,
        fix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_says_each_left_process_with_its_state_and_command() {
        assert_eq!(
            parsed("  4242 U    /usr/bin/xcrun simctl boot  X"),
            Some(Left {
                pid: 4242,
                state: "U".into(),
                command: "/usr/bin/xcrun simctl boot X".into()
            })
        );
        assert_eq!(parsed("garbage"), None);
    }
}

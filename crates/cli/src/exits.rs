//! A stopped candidate's processes that take long to exit. Its turn waits
//! for them with no bound (`candidates::confirmed`); past
//! `candidates::LONG_AFTER` it records here what is left, each pid with its
//! user, state and command, for as long as it waits, and `doctor` says each
//! wait whose turn is alive as a finding of the host for its watch
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
    pub user: String,
    /// Its state: `U` or `D` is uninterruptible I/O, `T` stopped.
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
    /// What is left that is killed hard each second.
    pub left: Vec<Left>,
    /// What is left that this user cannot kill: a setuid child, such as
    /// `/bin/ps`, which only its owner, or root, can end.
    pub unkillable: Vec<Left>,
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

/// Records that what candidate `id` leaves, `left`, of its `scope`, is still
/// there after `waited`.
pub(crate) fn waiting(
    id: &str,
    scope: &std::path::Path,
    left: &crate::candidates::Left,
    waited: Duration,
) -> Result<()> {
    let waiter = crate::candidates::identity(std::process::id())?
        .ok_or_else(|| anyhow::anyhow!("cannot identify the waiting process"))?;
    let record = Waiting {
        candidate: id.to_string(),
        lane: lane(id),
        since: crate::clock().saturating_sub(waited.as_secs()),
        waiter,
        held: left.held,
        scope: scope.to_string_lossy().into_owned(),
        left: listed(&left.pids),
        unkillable: listed(&left.unkillable),
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

/// Those of `pids` that still run, as `ps` lists them: a zombie has ended.
/// Soft: where `ps` cannot say, each is taken to run still.
pub(crate) fn running(pids: &[u32]) -> Vec<u32> {
    match said_by_ps(pids) {
        Some(listed) => listed
            .into_iter()
            .filter(|left| !left.state.starts_with('Z'))
            .map(|left| left.pid)
            .collect(),
        None => pids.to_vec(),
    }
}

/// `pids` as `ps` says them; one it no longer lists is left out.
fn listed(pids: &[u32]) -> Vec<Left> {
    said_by_ps(pids).unwrap_or_default()
}

fn said_by_ps(pids: &[u32]) -> Option<Vec<Left>> {
    if pids.is_empty() {
        return Some(vec![]);
    }
    let asked: Vec<String> = pids.iter().map(u32::to_string).collect();
    let out = crate::process::output(Command::new("ps").args([
        "-o",
        "pid=,user=,stat=,command=",
        "-p",
        &asked.join(","),
    ]))
    .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).lines().filter_map(parsed).collect())
}

fn parsed(line: &str) -> Option<Left> {
    let mut words = line.split_whitespace();
    let pid = words.next()?.parse().ok()?;
    let user = words.next()?.to_string();
    let state = words.next()?.to_string();
    Some(Left { pid, user, state, command: words.collect::<Vec<_>>().join(" ") })
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

/// The finding `record` is at `now`. One this user cannot kill is named
/// with what ends it, `sudo kill -9`, which is then its `fix`.
pub(crate) fn said(record: &Waiting, now: u64) -> Finding {
    let minutes = now.saturating_sub(record.since) / 60;
    let named = |left: &Left| {
        format!("pid {} ({}, {}) `{}`", left.pid, left.user, left.state, left.command)
    };
    let mut said: Vec<String> = record.left.iter().map(named).collect();
    said.extend(record.unkillable.iter().map(|left| {
        format!(
            "{}, which this user cannot kill: it needs `sudo kill -9 {}`",
            named(left),
            left.pid
        )
    }));
    let mut evidence = if said.is_empty() {
        "no process it can name is left".to_string()
    } else {
        said.join("; ")
    };
    if record.held {
        evidence.push_str(&format!("; its process scope {} is still held", record.scope));
    }
    evidence.push_str(&format!(
        "; pid {} keeps waiting, killing what it can hard each second",
        record.waiter.pid
    ));
    let pids = |lefts: &[Left]| lefts.iter().map(|left| left.pid.to_string()).collect::<Vec<_>>();
    let fix = if !record.unkillable.is_empty() {
        format!("sudo kill -9 {}", pids(&record.unkillable).join(" "))
    } else if !record.left.is_empty() {
        format!("ps -o pid,user,stat,wchan,command -p {}", pids(&record.left).join(","))
    } else {
        format!("sudo lsof {}", record.scope)
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
    fn ps_says_each_left_process_with_its_user_state_and_command() {
        assert_eq!(
            parsed("  4242 root   TN   /bin/ps axeww -o pid="),
            Some(Left {
                pid: 4242,
                user: "root".into(),
                state: "TN".into(),
                command: "/bin/ps axeww -o pid=".into()
            })
        );
        assert_eq!(parsed("garbage"), None);
    }

    /// A scope holder this user cannot kill is named with what ends it.
    #[test]
    fn an_unkillable_holder_is_named_with_sudo_kill() {
        let waiter = crate::candidates::identity(std::process::id()).unwrap().unwrap();
        let record = Waiting {
            candidate: "c9".into(),
            lane: lane("c9"),
            since: 1000,
            waiter,
            held: true,
            scope: "/w.processes".into(),
            left: vec![],
            unkillable: vec![Left {
                pid: 61330,
                user: "root".into(),
                state: "TN".into(),
                command: "/bin/ps axeww".into(),
            }],
        };
        let finding = said(&record, 1000 + 4 * 3600);
        assert!(finding.what.contains("240 min"), "{}", finding.what);
        assert!(
            finding.evidence.contains("pid 61330 (root, TN) `/bin/ps axeww`, which this user cannot kill: it needs `sudo kill -9 61330`"),
            "{}",
            finding.evidence
        );
        assert_eq!(finding.fix, "sudo kill -9 61330");
    }
}

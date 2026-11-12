//! The machine this host is on, as any account on it may see it. A machine
//! shared by several accounts holds a host of each — its own checkout list,
//! host kit, exclusive lanes and panel — and `doctor` names the host runners
//! of the other accounts from the process table alone, never their state,
//! socket or checkouts; and the afkconvoy first on PATH, where it is not the
//! release this account's host runner runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};

use afkconvoy_core::resources::Elsewhere;

use crate::doctor::{Finding, Kind, finding};
use crate::host_runner;

/// The host runner of another account on this machine, and what every
/// process of that account uses of it now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Runner {
    pub account: String,
    pub uid: u32,
    pub pid: u32,
    /// The file it was started from, as the process table says it.
    pub program: String,
    /// The cores the account's processes use: their `%CPU` summed, over 100.
    pub cores: f64,
    /// The memory they hold resident, in bytes.
    pub memory_bytes: u64,
    pub processes: usize,
}

/// The process table as `ps` says it to this account, read once a look:
/// what any account may read.
pub struct Table {
    text: String,
    me: u32,
}

impl Table {
    pub fn read() -> Result<Table> {
        let out = crate::process::output(Command::new("ps").args([
            "-axww",
            "-o",
            "uid=,user=,pid=,%cpu=,rss=,command=",
        ]))?;
        ensure!(out.status.success(), "`ps` ended {}", out.status);
        // SAFETY: getuid takes nothing, cannot fail and changes nothing.
        let me = unsafe { libc::getuid() };
        Ok(Table { text: String::from_utf8_lossy(&out.stdout).into_owned(), me })
    }

    /// The host runners of every account but this one on this machine.
    pub fn runners(&self) -> Vec<Runner> {
        runners(&self.text, self.me)
    }

    /// The other user account that holds the most resident memory, where it
    /// holds more than this one: memory pressure then is that account's.
    pub fn elsewhere(&self) -> Option<Elsewhere> {
        elsewhere(&self.text, self.me)
    }
}

/// The host runners of every account but this one on this machine, as `ps`
/// says them, or why it could not say.
pub fn other_runners(table: &Result<Table>) -> Result<Vec<Runner>> {
    match table {
        Ok(table) => Ok(table.runners()),
        Err(error) => Err(anyhow::anyhow!("{error:#}")),
    }
}

/// One row of the process table: uid, user, pid, %CPU, resident KiB, then
/// the command line.
struct Row<'a> {
    uid: u32,
    user: &'a str,
    pid: u32,
    cpu: f64,
    resident: u64,
    command: &'a str,
}

fn row(line: &str) -> Option<Row<'_>> {
    let mut rest = line.trim_start();
    let mut field = || {
        let (word, after) = rest.split_once(char::is_whitespace)?;
        rest = after.trim_start();
        Some(word)
    };
    let (uid, user, pid) = (field()?.parse().ok()?, field()?, field()?.parse().ok()?);
    let (cpu, resident) = (field()?.parse().ok()?, field()?.parse().ok()?);
    Some(Row { uid, user, pid, cpu, resident, command: rest.trim_end() })
}

/// The host runners in the process table `table` of every account but `me`'s,
/// each with what its account's processes use, by pid.
fn runners(table: &str, me: u32) -> Vec<Runner> {
    let rows: Vec<Row> = table.lines().filter_map(row).collect();
    let mut found: Vec<Runner> = rows
        .iter()
        .filter(|row| row.uid != me)
        .filter_map(|row| {
            let program = row.command.strip_suffix(" host-runner")?;
            let named = Path::new(program).file_name().is_some_and(|name| name == "afkconvoy");
            named.then(|| {
                let of_account = rows.iter().filter(|other| other.uid == row.uid);
                let (cpu, resident, processes) = of_account
                    .fold((0.0, 0, 0), |(cpu, resident, n), other| {
                        (cpu + other.cpu, resident + other.resident, n + 1)
                    });
                Runner {
                    account: row.user.to_string(),
                    uid: row.uid,
                    pid: row.pid,
                    program: program.to_string(),
                    cores: (cpu / 10.0).round() / 10.0,
                    memory_bytes: resident * 1024,
                    processes,
                }
            })
        })
        .collect();
    found.sort_by_key(|runner| runner.pid);
    found
}

/// In the process table `table`, the user account but `me`'s holding the
/// most resident memory, where it holds more than `me`'s.
fn elsewhere(table: &str, me: u32) -> Option<Elsewhere> {
    let mut accounts: Vec<(u32, &str, u64, usize)> = vec![];
    for row in table.lines().filter_map(row) {
        match accounts.iter_mut().find(|(uid, ..)| *uid == row.uid) {
            Some((_, _, resident, processes)) => {
                *resident += row.resident * 1024;
                *processes += 1;
            }
            None => accounts.push((row.uid, row.user, row.resident * 1024, 1)),
        }
    }
    let held: Vec<(u32, u64)> = accounts.iter().map(|(uid, _, bytes, _)| (*uid, *bytes)).collect();
    let at = afkconvoy_core::resources::elsewhere(&held, me)?;
    let (uid, account, memory_bytes, processes) = accounts[at];
    Some(Elsewhere { account: account.to_string(), uid, memory_bytes, processes })
}

/// `doctor`'s `host.other_accounts`: each of `read`, or {error}.
pub fn json(read: &Result<Vec<Runner>>) -> Value {
    match read {
        Ok(runners) => json!(runners),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// Where this account has no host runner of its own (`runs`), one finding of
/// each host runner of another account: the fleet the reader looks for may
/// run under that account.
pub fn findings(read: &Result<Vec<Runner>>, runs: bool) -> Vec<Finding> {
    let Ok(runners) = read else { return vec![] };
    if runs {
        return vec![];
    }
    runners
        .iter()
        .map(|runner| {
            let Runner { account, uid, pid, program, cores, memory_bytes, processes } = runner;
            finding(
                Kind::OtherAccount,
                json!(["other-account", uid, pid]),
                format!(
                    "this account has no host runner, and the host runner of the account \
                     {account} (pid {pid}) works on this machine under another account: the \
                     fleet you look for may be that account's"
                ),
                format!(
                    "the process table: {program} host-runner, pid {pid}, uid {uid}; the \
                     account's {processes} processes use {cores} cores and {:.1} GiB resident",
                    *memory_bytes as f64 / (1u64 << 30) as f64
                ),
                format!("su - {account} -c 'afkconvoy doctor --all'"),
            )
        })
        .collect()
}

/// The afkconvoy first on PATH, where it is not of the release this account's
/// host runner runs — or, where none runs, the one a `run` of this afkconvoy
/// would start: a command typed in this shell runs another than the fleet's.
pub fn path_findings() -> Vec<Finding> {
    let Some(first) = first_on_path() else { return vec![] };
    let (expected, of) = match host_runner::runs() {
        true => match host_runner::version() {
            Some(version) => (version, "runs"),
            None => return vec![],
        },
        false => (crate::VERSION.to_string(), "would start"),
    };
    let ours = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    let said = match std::fs::canonicalize(&first).ok() == ours && ours.is_some() {
        true => Ok(crate::VERSION.to_string()),
        false => host_runner::version_of(&first),
    };
    let at = first.display();
    let what = match &said {
        Ok(version) if *version == expected => return vec![],
        Ok(version) => format!(
            "afkconvoy {version}, at {at}, is first on PATH, and this account's host runner \
             {of} afkconvoy {expected}: an `afkconvoy` typed in this shell is not the fleet's"
        ),
        Err(error) => format!(
            "the afkconvoy at {at} is first on PATH and does not say its version ({error:#}); \
             this account's host runner {of} afkconvoy {expected}"
        ),
    };
    vec![finding(
        Kind::PathStale,
        json!(["path-stale", first, said.ok()]),
        what,
        format!("PATH={}", std::env::var("PATH").unwrap_or_default()),
        "which -a afkconvoy".to_string(),
    )]
}

/// The first `afkconvoy` file on PATH that may be run.
fn first_on_path() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join("afkconvoy")).find(|file| {
        std::fs::metadata(file)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
  501 h          3264   2.0  40960 /Users/h/homebrew/bin/afkconvoy host-runner
  501 h          3300  50.0 102400 claude --model opus
  502 felix      4100   1.5  20480 /opt/homebrew/bin/afkconvoy host-runner
  502 felix      4200 148.5 1048576 cargo build
  502 felix      4300   0.0   1024 /opt/homebrew/bin/afkconvoy doctor
    0 root          1   0.1   8192 /sbin/launchd
";

    #[test]
    fn another_accounts_host_runner_is_named_with_what_its_account_uses() {
        let found = runners(TABLE, 501);
        assert_eq!(
            found,
            vec![Runner {
                account: "felix".into(),
                uid: 502,
                pid: 4100,
                program: "/opt/homebrew/bin/afkconvoy".into(),
                cores: 1.5,
                memory_bytes: (20480 + 1048576 + 1024) * 1024,
                processes: 3,
            }]
        );
    }

    #[test]
    fn the_account_holding_the_most_memory_is_named_where_it_is_not_this_one() {
        let table = "\
  501 sunfmin     100   0.0 2097152 /Users/sunfmin/homebrew/bin/afkconvoy host-runner
  502 felix       200  80.0 16777216 /opt/homebrew/bin/limactl hostagent colima
  502 felix       300  20.0 4194304 launchd_sim
    0 root          1   0.1 41943040 /sbin/launchd
";
        assert_eq!(
            elsewhere(table, 501),
            Some(Elsewhere {
                account: "felix".into(),
                uid: 502,
                memory_bytes: 20 << 30,
                processes: 2,
            })
        );
        assert_eq!(elsewhere(table, 502), None);
        assert_eq!(elsewhere(TABLE, 501).map(|held| held.processes), Some(3));
        assert_eq!(elsewhere(TABLE, 502), None, "this account holds more than 501, root is none");
    }

    #[test]
    fn this_accounts_own_host_runner_is_none_of_them() {
        assert!(runners(TABLE, 502).iter().all(|runner| runner.uid != 502));
        assert_eq!(runners(TABLE, 502).len(), 1);
    }

    #[test]
    fn another_accounts_host_runner_is_a_finding_only_where_this_account_has_none() {
        let read = Ok(runners(TABLE, 501));
        assert!(findings(&read, true).is_empty());
        let found = findings(&read, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::OtherAccount);
        assert!(found[0].what.contains("felix (pid 4100)"), "{}", found[0].what);
        assert_eq!(found[0].fix, "su - felix -c 'afkconvoy doctor --all'");
    }
}

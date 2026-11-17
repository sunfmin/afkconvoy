//! Hot workers, as `run` finds them (docs/adr/0019): every `SAMPLE_EVERY`
//! seconds it reads what each worker's processes take of the CPU and memory,
//! and what they wrote to disk — from each process's own usage, as `ps` and
//! the kernel say it, with no root — and every `LISTED_FOR` samples what each
//! worker's worktree holds, by the train worktree's measure; and the episode
//! rule (`afkconvoy_core::hot`) says which turned hot and which
//! stopped being hot. Each turn is a line of the log.
//!
//! A worker's processes are those started with its `TMPDIR`, the one of its
//! worktree (`worker_tmp`), and every process under one of them: a process
//! whose environment cannot be read — a system binary — is counted through
//! the one it was started under. A server shared by the host that the worker
//! started is not (`shared`). The landing train's worktree is no worker's.
//! What runs under one of its `afkconvoy exclusive` commands — a running
//! ticket's process, and every one under it — is said apart (`exclusive`):
//! the queue bounds that heavy work, and its memory counts against the limit
//! only while the host's memory pressure is at level 2 or above.
//!
//! As a worker turns hot, what shows it is taken down there and then, on a
//! thread of its own (`evidence`), and the `run` is woken. The resource
//! reading becomes a doctor finding for the host watch's next look.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use afkconvoy_core::exclusive::Ticket;
use afkconvoy_core::hot::{
    CPU_HELD, Episode, Limits, ORPHAN_AFTER, SAMPLE_EVERY, STALL_HELD, Sample, Turned,
    WRITTEN_WITHIN, Watch, Why, millicores,
};
use afkconvoy_core::landing::{Built, OUTGROWN_FLOOR, OUTGROWN_TIMES};
use serde::{Deserialize, Serialize};

use crate::evidence::{self, Taken};
use crate::git::Repo;
use crate::host_settings;
use crate::log::What;
use crate::measure;
use crate::orca::{Source, Worktree};
use crate::stall::{self, Idle, Stall};
use crate::stop::Stop;
use crate::wake::Waker;
use crate::worker_tmp;

/// How many samples go by before the worktrees are asked of orca again, and
/// what each holds is measured again: that is slow.
const LISTED_FOR: u32 = 6;
/// How many samples of a worker's resident memory are kept for the evidence:
/// the last five minutes'.
const RESIDENT_KEPT: usize = 30;

/// What the host says of the limits, where it says any: its host settings
/// `--hot-cores`, `--hot-memory`, `--hot-written` and `--hot-spawns`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Said {
    pub millicores: Option<u64>,
    pub resident: Option<u64>,
    pub written: Option<u64>,
    pub spawns: Option<u64>,
}

/// The limits on this host for the fleet of the checkout `dir`: its shares
/// (`Limits::of_host`) — of the disk `dir` is on, free over its floor
/// (`disk::headroom`) — but for what it says.
pub fn limits(said: Said, dir: &Path) -> Limits {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get() as u64);
    let headroom = crate::admission::disk(dir).ok().map(|disk| {
        let settings = crate::host_settings::of_host().unwrap_or_default();
        crate::disk::headroom(disk, crate::disk::reserve_of(&settings, &crate::disk::listed()))
    });
    limits_from(said, cores, memory(), headroom)
}

/// The limits derived from an existing host reading.
pub fn limits_from(said: Said, cores: u64, memory: Option<u64>, disk: Option<u64>) -> Limits {
    let shares = Limits::of_host(cores, memory.unwrap_or(u64::MAX), disk.unwrap_or(u64::MAX));
    Limits {
        millicores: said.millicores.unwrap_or(shares.millicores),
        resident: said.resident.unwrap_or(shares.resident),
        written: said.written.unwrap_or(shares.written),
        spawns: said.spawns.unwrap_or(shares.spawns),
    }
}

/// The bytes the process `pid` has written to disk since it started, as the
/// kernel counts them for it; none where they cannot be read — a process of
/// another user, or one gone.
#[cfg(target_os = "macos")]
pub(crate) fn written_by(pid: u32) -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `usage` is a `rusage_info_v2`, the flavor asked for, and the
    // kernel writes no more than one of it.
    let read = unsafe {
        libc::proc_pid_rusage(pid as libc::c_int, libc::RUSAGE_INFO_V2, usage.as_mut_ptr().cast())
    };
    if read != 0 {
        return 0;
    }
    // SAFETY: zeroed, then written by the kernel: every field is a number.
    unsafe { usage.assume_init() }.ri_diskio_byteswritten
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn written_by(pid: u32) -> u64 {
    let io = std::fs::read_to_string(format!("/proc/{pid}/io")).unwrap_or_default();
    let line = io.lines().find_map(|line| line.strip_prefix("write_bytes:"));
    line.and_then(|bytes| bytes.trim().parse().ok()).unwrap_or(0)
}

/// The directory the process `pid` works in, as the kernel says it, every
/// link in it followed; none where it cannot be read — a process of another
/// user, or one gone.
#[cfg(target_os = "macos")]
fn works_in(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is a `proc_vnodepathinfo`, the flavor asked for, and
    // `size` is its size: the kernel writes no more than that.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    // SAFETY: zeroed, then written by the kernel: every field is a number.
    let info = unsafe { info.assume_init() };
    let path: Vec<u8> = info.pvi_cdir.vip_path.iter().flatten().map(|&byte| byte as u8).collect();
    let path = &path[..path.iter().position(|&byte| byte == 0)?];
    (!path.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(path)))
}

#[cfg(not(target_os = "macos"))]
fn works_in(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The working directory of every process this host can see.
///
/// A process whose directory cannot be read is left out: that is not evidence
/// it works somewhere else. An error from the process list removes nothing
/// that was waiting on this answer.
pub(crate) fn working_directories() -> anyhow::Result<Vec<PathBuf>> {
    Ok(working_directories_of(&listed_checked()?))
}

/// `working_directories`, of the processes `procs` lists: a listing its
/// caller has just made.
pub(crate) fn working_directories_of(procs: &[Proc]) -> Vec<PathBuf> {
    procs.iter().filter_map(|proc| works_in(proc.pid)).collect()
}

/// The stopped orphans working in `worktree`, and every process under one:
/// each stopped (`T`), handed to pid 1 — what paused it is gone — and
/// working in the worktree. Nothing is left to resume one: it stays stopped
/// for good, and a SIGTERM to it stays pending (sunfmin/control#837).
pub(crate) fn stopped_orphans(procs: &[Proc], worktree: &Path) -> anyhow::Result<Vec<u32>> {
    let out = crate::process::output(crate::census::command("ps").args(["-axo", "pid=,stat="]))?;
    anyhow::ensure!(out.status.success(), "cannot read which processes are stopped: ps failed");
    let stopped: HashSet<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|row| {
            let mut fields = row.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            fields.next()?.starts_with('T').then_some(pid)
        })
        .collect();
    let mut found: Vec<u32> = procs
        .iter()
        .filter(|proc| {
            proc.ppid == 1
                && stopped.contains(&proc.pid)
                && proc
                    .cwd
                    .as_ref()
                    .is_some_and(|cwd| occupies(std::slice::from_ref(cwd), worktree))
        })
        .map(|proc| proc.pid)
        .collect();
    let mut next = 0;
    while next < found.len() {
        let pid = found[next];
        next += 1;
        for child in procs.iter().filter(|proc| proc.ppid == pid) {
            if !found.contains(&child.pid) {
                found.push(child.pid);
            }
        }
    }
    Ok(found)
}

/// Processes whose current directory is in this worktree, without their environment.
pub(crate) fn working_in(worktree: &Path) -> anyhow::Result<Vec<String>> {
    Ok(listed_checked()?
        .into_iter()
        .filter_map(|proc| {
            let cwd = works_in(proc.pid)?;
            occupies(&[cwd], worktree).then(|| {
                format!("{} {}", proc.pid, proc.line.split_whitespace().next().unwrap_or("unknown"))
            })
        })
        .collect())
}

/// Whether any process's working directory is `worktree` or a path inside it.
pub(crate) fn occupies(directories: &[PathBuf], worktree: &Path) -> bool {
    let worktree = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    directories.iter().any(|cwd| {
        // The kernel's path and the path we recorded can disagree on a
        // symlink (`/var` and `/private/var`). Compare the real paths.
        let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
        cwd == worktree || cwd.starts_with(&worktree)
    })
}

/// The bytes of memory this host has; `None` where it cannot be read. On
/// macOS it is asked once a process: it does not change while one runs.
pub fn memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        static MEMSIZE: OnceLock<Option<u64>> = OnceLock::new();
        return *MEMSIZE.get_or_init(|| sysctl("hw.memsize"));
    }
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib.saturating_mul(1024))
}

fn sysctl(name: &str) -> Option<u64> {
    let out = crate::process::output(crate::census::command("sysctl").args(["-n", name])).ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// A worker that is hot now, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotNow {
    /// `#7`, or the name of its worktree where it is linked to no issue.
    pub worker: String,
    pub episode: Episode,
    /// Its worktree, and the issue orca links it to: what its watch is
    /// started on.
    pub path: PathBuf,
    pub branch: String,
    pub issue: Option<u64>,
    /// Why it turned hot, in words, under the limits in force as it did.
    pub why: String,
}

impl HotNow {
    pub fn worktree(&self) -> Worktree {
        Worktree::at(self.path.clone(), self.branch.clone())
    }
}

/// The workers that are hot now, shared between the sampler and the panel.
pub type Hot = Arc<Mutex<Vec<HotNow>>>;

/// One process, as `ps` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proc {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    /// Resident memory, in bytes.
    pub(crate) resident: u64,
    /// The CPU it has taken since it started, in milliseconds.
    pub(crate) cpu_ms: u64,
    /// The share of a core it takes now, as `ps` says it, in hundredths of
    /// a percent: what admission apportions the load by (docs/adr/0050).
    pub(crate) cpu_share: u64,
    /// The bytes it has written to disk since it started.
    pub(crate) written: u64,
    /// Its command line, then its environment, where that can be read.
    pub(crate) line: String,
    /// The directory it works in, read of a process handed to pid 1 alone:
    /// what tells a server shared by the host from a process a worker left
    /// behind (`shared`).
    pub(crate) cwd: Option<PathBuf>,
}

/// Every process of this host, as `ps` says them, with what each wrote.
pub(crate) fn processes() -> Vec<Proc> {
    let mut procs = listed();
    for proc in &mut procs {
        proc.written = written_by(proc.pid);
    }
    procs
}

/// Every process of this host, as `ps` says them, and nothing more.
pub(crate) fn listed() -> Vec<Proc> {
    listed_checked().unwrap_or_default()
}

/// A failed observation must never certify that an old writer is gone.
pub(crate) fn listed_checked() -> anyhow::Result<Vec<Proc>> {
    let out = crate::process::output(crate::census::command("ps").args([
        "axeww",
        "-o",
        "pid=,ppid=,pcpu=,rss=,time=,command=",
    ]))?;
    anyhow::ensure!(out.status.success(), "cannot confirm worker termination: ps failed");
    let mut rows: Vec<_> = String::from_utf8_lossy(&out.stdout).lines().filter_map(parse).collect();
    anyhow::ensure!(
        !rows.is_empty(),
        "cannot confirm worker termination: ps returned no processes"
    );
    // Every caller of tree needs the same shared-server exclusion, including
    // continuation's checked observation, not only the hot-worker sampler.
    for proc in &mut rows {
        if proc.ppid == 1 {
            proc.cwd = works_in(proc.pid);
        }
    }
    Ok(rows)
}

/// Every process of this host that has a terminal, and every process above
/// one, each with its command line and its environment: what says how a
/// worker's harness stands at its terminal (`foreground::holds`), which
/// judges only a process with a terminal. Whether a process is a worker's
/// (`tree`) follows from it and the processes above it alone, so a listing
/// that holds every process above each it holds judges it as the whole
/// table does. Read from the kernel, not `ps`: the full listing, every
/// environment read, took about 160 ms on a loaded host, and a slow-gate
/// run made 1,443 of them only to type a line or look at a harness; this
/// takes about 3 ms (sunfmin/control#1158). A process this user cannot
/// read is another user's, which no worker here starts: the processes
/// above it are left out, as no worker's. Errors: the kernel listed none.
#[cfg(target_os = "macos")]
pub(crate) fn on_terminals() -> anyhow::Result<Vec<Proc>> {
    /// No terminal, as the kernel says a device.
    const NODEV: u32 = u32::MAX;
    let mut pids = vec![0 as libc::pid_t; 4096];
    let listed = loop {
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `pids` holds `bytes` bytes of pids, as many as asked for.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        anyhow::ensure!(
            n > 0,
            "cannot list this host's processes: {}",
            std::io::Error::last_os_error()
        );
        let n = n as usize;
        if n < pids.len() {
            break n;
        }
        pids.resize(pids.len() * 2, 0);
    };
    // pid → its parent, and whether it has a terminal: of every process
    // this user may read.
    let mut table: HashMap<u32, (u32, bool)> = HashMap::new();
    for &pid in pids.get(..listed).unwrap_or_default() {
        let Ok(pid) = u32::try_from(pid) else { continue };
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` is a `proc_bsdinfo`, the flavor asked for, and
        // `size` is its size: the kernel writes no more than that.
        let got = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if got != size {
            continue;
        }
        // SAFETY: a complete kernel write of numeric fields and byte arrays.
        let info = unsafe { info.assume_init() };
        if info.pbi_status == libc::SZOMB {
            continue;
        }
        table.insert(pid, (info.pbi_ppid, info.e_tdev != NODEV));
    }
    let mut asked: BTreeSet<u32> = BTreeSet::new();
    for (&pid, &(_, terminal)) in &table {
        if !terminal {
            continue;
        }
        let mut at = pid;
        while at > 1 && asked.insert(at) {
            match table.get(&at) {
                Some(&(ppid, _)) => at = ppid,
                // Gone, or another user's, which no worker here starts:
                // nothing above it is a worker's either.
                None => break,
            }
        }
    }
    let mut rows = vec![];
    for pid in asked {
        let Some(&(ppid, _)) = table.get(&pid) else { continue };
        let line = said(pid).unwrap_or_default();
        let cwd = (ppid == 1).then(|| works_in(pid)).flatten();
        rows.push(Proc { pid, ppid, resident: 0, cpu_ms: 0, cpu_share: 0, written: 0, line, cwd });
    }
    Ok(rows)
}

/// The arguments, then the environment, `pid` was started with, each one
/// after a space, as `ps axeww` says them; `None` where they cannot be
/// read: a process of another user, or one gone.
#[cfg(target_os = "macos")]
fn said(pid: u32) -> Option<String> {
    let mut name = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    // SAFETY: a size query: no buffer is written.
    let asked = unsafe {
        libc::sysctl(name.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0)
    };
    if asked != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` holds `size` bytes, as many as the kernel may write.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    let buffer = buffer.get(..size)?;
    // How many arguments, then the path it executed, then what it was given.
    let argc = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?);
    let rest = buffer.get(4..)?;
    let path_end = rest.iter().position(|&byte| byte == 0)?;
    let given = rest.get(path_end..)?;
    let start = given.iter().position(|&byte| byte != 0)?;
    let mut words = given.get(start..)?.split(|&byte| byte == 0);
    let argc = usize::try_from(argc).ok()?;
    let mut said: Vec<String> =
        words.by_ref().take(argc).map(|word| String::from_utf8_lossy(word).into_owned()).collect();
    // The environment ends at the first empty string: what follows pads.
    said.extend(
        words
            .take_while(|word| !word.is_empty())
            .map(|word| String::from_utf8_lossy(word).into_owned()),
    );
    Some(said.join(" "))
}

/// Every process of this host, where it is not read from the kernel.
#[cfg(not(target_os = "macos"))]
pub(crate) fn on_terminals() -> anyhow::Result<Vec<Proc>> {
    listed_checked()
}

/// Every process of this host with the share of a core it takes now, and,
/// for its `line`, the program it runs alone, without its arguments or its
/// environment: the light reading the host runner averages the exclusive
/// queue's share of the CPU by, every few seconds while it runs
/// (`queue_share`). Errors: `ps` failed, or listed nothing.
pub(crate) fn cpu_table() -> anyhow::Result<Vec<Proc>> {
    let out = crate::process::output(crate::census::command("ps").args([
        "ax",
        "-o",
        "pid=,ppid=,pcpu=,rss=,time=,comm=",
    ]))?;
    anyhow::ensure!(out.status.success(), "ps failed");
    let rows: Vec<_> = String::from_utf8_lossy(&out.stdout).lines().filter_map(parse).collect();
    anyhow::ensure!(!rows.is_empty(), "ps listed no processes");
    Ok(rows)
}

/// One row of `ps`: pid, ppid, share of a core, resident KiB, CPU time,
/// then the rest. A share that is no number counts as none.
fn parse(row: &str) -> Option<Proc> {
    let mut rest = row.trim_start();
    let mut field = || {
        let (word, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        rest = after.trim_start();
        word
    };
    let pid = field().parse().ok()?;
    let ppid = field().parse().ok()?;
    let share = field().parse::<f64>().ok().filter(|share| share.is_finite() && *share >= 0.0);
    let cpu_share = share.map_or(0, |share| (share * 100.0).round() as u64);
    let resident = field().parse::<u64>().ok()?.saturating_mul(1024);
    let cpu_ms = cpu_time(field())?;
    let line = rest.to_string();
    Some(Proc { pid, ppid, resident, cpu_ms, cpu_share, written: 0, line, cwd: None })
}

/// `[[dd-]hh:]mm:ss[.cc]`, as `ps` says CPU time → milliseconds.
fn cpu_time(said: &str) -> Option<u64> {
    let (days, clock) = match said.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, said),
    };
    let (whole, hundredths) = match clock.split_once('.') {
        Some((whole, fraction)) => (whole, format!("{fraction:0<2}")[..2].parse::<u64>().ok()?),
        None => (clock, 0),
    };
    let mut seconds = days * 86_400;
    let mut unit = 1;
    for part in whole.rsplit(':') {
        seconds += part.parse::<u64>().ok()? * unit;
        unit *= 60;
    }
    Some(seconds * 1000 + hundredths * 10)
}

/// Whether `proc` was started with `tmp` for its `TMPDIR`.
pub(crate) fn started_with(proc: &Proc, tmp: &str) -> bool {
    let needle = format!(" TMPDIR={tmp}");
    proc.line.match_indices(&needle).any(|(at, _)| {
        let after = &proc.line[at + needle.len()..];
        let after = after.strip_prefix('/').unwrap_or(after);
        after.is_empty() || after.starts_with(' ')
    })
}

/// Whether `proc`, started with the `TMPDIR` of the worker in `worktree`, is
/// a server shared by the host that the worker started: handed to pid 1 —
/// what started it is gone — and working outside the worktree, as a daemon
/// does. sccache's server works in `/`, for every worktree of the host, and
/// each compile it runs for a worker is started with that worker's
/// `TMPDIR`: counted to it. One handed to pid 1 that works in the worktree —
/// a test whose runner ended — is the worker's, left behind.
pub(crate) fn shared(proc: &Proc, worktree: &Path) -> bool {
    proc.ppid == 1 && proc.cwd.as_ref().is_some_and(|cwd| !cwd.starts_with(worktree))
}

/// The processes of the worker in the worktree `worktree`: each started with
/// its `TMPDIR` but a server shared by the host (`shared`), and each under
/// one of those.
pub(crate) fn tree<'a>(procs: &'a [Proc], worktree: &Path) -> Vec<&'a Proc> {
    let tmp = worker_tmp::of(worktree).display().to_string();
    // A process's directory is said with every link followed.
    let worktree = worktree.canonicalize().unwrap_or_else(|_| worktree.to_path_buf());
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for proc in procs {
        children.entry(proc.ppid).or_default().push(proc);
    }
    let mut found: Vec<&Proc> =
        procs.iter().filter(|proc| started_with(proc, &tmp) && !shared(proc, &worktree)).collect();
    let mut seen: std::collections::HashSet<u32> = found.iter().map(|proc| proc.pid).collect();
    let mut next = 0;
    while next < found.len() {
        let pid = found[next].pid;
        next += 1;
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child.pid) {
                found.push(child);
            }
        }
    }
    found
}

/// The pids of `procs` the fleets of this host started: each started with a
/// worker's `TMPDIR` (`worker_tmp::is_workers`), or one in a worktree's own
/// (`TMP_DIR`) — or under either, as a gate sets one — where `ours` says the
/// fleet put a worker, and each under one of those, or left behind by one. A
/// worker's harness, its gates and its tests are; what a human runs in a
/// checkout of their own is not.
pub(crate) fn fleets(procs: &[Proc], ours: impl Fn(&Path) -> bool) -> HashSet<u32> {
    let mut asked: HashMap<PathBuf, bool> = HashMap::new();
    let mut started = |proc: &Proc| {
        let dirs: Vec<&str> = tmp_dirs(&proc.line).collect();
        dirs.iter().any(|dir| worker_tmp::is_workers(dir))
            || dirs
                .into_iter()
                .filter_map(worktree_of)
                .any(|worktree| *asked.entry(worktree.clone()).or_insert_with(|| ours(&worktree)))
    };
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for proc in procs {
        children.entry(proc.ppid).or_default().push(proc);
    }
    let mut found: Vec<u32> = procs.iter().filter(|proc| started(proc)).map(|p| p.pid).collect();
    let mut seen: HashSet<u32> = found.iter().copied().collect();
    let mut next = 0;
    while let Some(pid) = found.get(next).copied() {
        next += 1;
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child.pid) {
                found.push(child.pid);
            }
        }
    }
    seen
}

/// Each `TMPDIR` the line of `ps` names, its command's words then its
/// environment.
fn tmp_dirs(line: &str) -> impl Iterator<Item = &str> {
    line.split(' ').filter_map(|word| word.strip_prefix("TMPDIR=")).filter(|dir| !dir.is_empty())
}

/// The worktree whose own `TMPDIR` holds `dir`; `None` where none does.
fn worktree_of(dir: &str) -> Option<PathBuf> {
    let needle = format!("/{}", worker_tmp::TMP_DIR);
    let (at, _) = dir.match_indices(&needle).find(|(at, _)| {
        let after = &dir[at + needle.len()..];
        after.is_empty() || after.starts_with('/')
    })?;
    (at > 0).then(|| PathBuf::from(&dir[..at]))
}

/// One worker the sampler watches.
struct Watched {
    worker: String,
    worktree: Worktree,
    issue: Option<u64>,
    watch: Watch,
    /// The CPU each of its processes had taken, and the bytes each had
    /// written, at the sample before; `None` before its first.
    before: Option<HashMap<u32, (u64, u64)>>,
    /// Its processes at the last sample, and the one of them that took the
    /// most CPU since the one before — or, taking none, held the most.
    pids: Vec<u32>,
    hottest: Option<u32>,
    /// What they held resident at each of the last samples, oldest first.
    resident: VecDeque<(u64, u64)>,
    /// Its stall, as last found: `None` where none was.
    stall: Option<Stall>,
    /// What each of the last `SPAWN_SAMPLES` samples found of process
    /// creation (`Created`), oldest first.
    created: VecDeque<Created>,
    /// The commands that created the processes each of the last samples
    /// found, by how many each: what its issue names (`spawners_of`).
    spawners: VecDeque<Vec<(String, u64)>>,
    /// Each process a command of it left behind (`orphans`), by when it was
    /// first seen so, and whether it has been told to end.
    orphans: BTreeMap<u32, Orphan>,
}

/// How many samples a worker's share of the host's process creation is read
/// over: the span it must be held for, `CPU_HELD`. A process that lives ten
/// milliseconds is on the list of one sample in a few: the share is of what
/// those samples found together.
const SPAWN_SAMPLES: usize = (CPU_HELD / SAMPLE_EVERY) as usize;

/// What one sample found of process creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Created {
    /// The processes the host created since the sample before, by its
    /// counter.
    host: u64,
    /// The processes of the host that were created then and are still here,
    /// and those of them that are the worker's.
    alive: u64,
    mine: u64,
    /// The milliseconds the sample took in.
    wall_ms: u64,
}

/// A process a command of a worker left behind: handed to pid 1, working in
/// its worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Orphan {
    since: u64,
    /// When it was told to end, where it was.
    told: Option<u64>,
}

/// The processes a second a worker created, from what the last samples found
/// of it: the host's creation over the time they took, by the share of the
/// processes created in it and still here that were the worker's. `None`:
/// no sample found a process created in it, or the time is none.
fn spawn_rate(window: &VecDeque<Created>) -> Option<u64> {
    let (host, alive, mine, wall_ms) = window.iter().fold((0u64, 0u64, 0u64, 0u64), |sum, c| {
        (sum.0 + c.host, sum.1 + c.alive, sum.2 + c.mine, sum.3 + c.wall_ms)
    });
    if alive == 0 || wall_ms == 0 {
        return None;
    }
    let made = host as f64 * mine as f64 / alive as f64;
    Some((made * 1000.0 / wall_ms as f64).round() as u64)
}

/// A process's command, without its environment: the words of its line up
/// to the first that names a variable, cut to 160 characters.
fn command_of(proc: &Proc) -> String {
    let words: Vec<&str> = proc
        .line
        .split(' ')
        .take_while(|word| {
            !word.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            })
        })
        .collect();
    words.join(" ").chars().take(160).collect()
}

impl Watched {
    fn new(worker: String, worktree: Worktree, issue: Option<u64>) -> Watched {
        Watched {
            worker,
            worktree,
            issue,
            watch: Watch::default(),
            before: None,
            pids: vec![],
            hottest: None,
            resident: VecDeque::new(),
            stall: None,
            created: VecDeque::new(),
            spawners: VecDeque::new(),
            orphans: BTreeMap::new(),
        }
    }
}

/// What a worker's processes took in one sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Took {
    /// The CPU since the sample before, in milliseconds; `None` at its first.
    cpu_ms: Option<u64>,
    /// The bytes written since the sample before; `None` at its first.
    written: Option<u64>,
    /// The bytes resident.
    resident: u64,
    /// The bytes of `resident` held under an exclusive command.
    exclusive: u64,
}

/// What a worker's processes `procs` took, those of `exclusive` held under an
/// exclusive command.
fn took(watched: &mut Watched, procs: &[&Proc], exclusive: &BTreeSet<u32>) -> Took {
    let now: HashMap<u32, (u64, u64)> =
        procs.iter().map(|proc| (proc.pid, (proc.cpu_ms, proc.written))).collect();
    // A process new since the sample before took all it has since then.
    let since = |of: fn(&(u64, u64)) -> u64| {
        watched.before.as_ref().map(|before| {
            let one = |(pid, taken): (&u32, &(u64, u64))| {
                of(taken).saturating_sub(before.get(pid).map_or(0, of))
            };
            now.iter().map(one).sum()
        })
    };
    let (cpu_ms, written) = (since(|taken| taken.0), since(|taken| taken.1));
    let busiest = watched.before.as_ref().and_then(|before| {
        let taken = now.iter().map(|(pid, (ms, _))| {
            (ms.saturating_sub(before.get(pid).map_or(0, |taken| taken.0)), *pid)
        });
        taken.filter(|(ms, _)| *ms > 0).max().map(|(_, pid)| pid)
    });
    let largest = procs.iter().max_by_key(|proc| proc.resident).map(|proc| proc.pid);
    watched.hottest = busiest.or(largest);
    watched.pids = procs.iter().map(|proc| proc.pid).collect();
    watched.before = Some(now);
    Took {
        cpu_ms,
        written,
        resident: procs.iter().map(|proc| proc.resident).sum(),
        exclusive: procs
            .iter()
            .filter(|proc| exclusive.contains(&proc.pid))
            .map(|proc| proc.resident)
            .sum(),
    }
}

/// What was known of `watched` as it turned hot in `episode`, under `limits`.
fn known(watched: &Watched, episode: &Episode, limits: &Limits) -> Taken {
    Taken {
        worker: watched.worker.clone(),
        since: episode.since,
        why: why_said(episode.why, limits),
        held: held_said(episode),
        pids: watched.pids.clone(),
        hottest: watched.hottest,
        resident: watched.resident.iter().copied().collect(),
        stall: watched.stall.as_ref().filter(|_| episode.why == Why::Stalled).map(Stall::said),
        spawns: (episode.why == Why::Spawns)
            .then(|| (episode.spawns, spawners_of(&watched.spawners))),
        issue: watched.issue,
    }
}

/// What the worktree at `path` holds, by the train worktree's measure, with
/// its fresh build: `None` while none is on record — what it built before
/// its first gate run is no measure of what it should hold — or where it
/// cannot be read.
fn held(path: &std::path::Path) -> Option<Built> {
    let repo = Repo::new(path.to_path_buf());
    let fresh = measure::fresh(&repo).ok()??;
    Some(Built { built: measure::built(&repo).ok()?, fresh: Some(fresh) })
}

/// What the sampler keeps between samples.
struct Sampler {
    /// What the host settings said of the limits at the last sample, and the
    /// limits that came to on this host.
    said: Said,
    limits: Limits,
    /// The worktrees of the workers, as orca last listed them, each with its
    /// worker's name and issue.
    listed: Vec<(PathBuf, String, Worktree, Option<u64>)>,
    samples: u32,
    /// The workers watched, by their worktrees.
    watched: BTreeMap<PathBuf, Watched>,
    /// Samples carried over from the preceding host runner.
    resumed: BTreeMap<PathBuf, Watch>,
    /// Since when each process of the workers has been idle.
    idle: Idle,
    /// The host's process id counter at the sample before.
    counter: Option<afkconvoy_host_reading::spawn::Counter>,
}

/// What one sample of the workers found.
struct Found {
    /// Each worker that turned hot or stopped being hot, as a line of the log.
    turned: Vec<What>,
    /// What is to be taken down of each worker that turned hot.
    hot_now: Vec<(Worktree, Taken)>,
    /// The processes a worker's commands left behind that were told to end:
    /// the worker, its issue, and each process in words.
    ended: Vec<(String, Option<u64>, Vec<String>)>,
}

/// What one sample found of the creation of the processes `counted` of one
/// worker among all `procs`, the host's counter read `(before, now)`.
fn created(
    procs: &[Proc],
    counted: &[&Proc],
    (before, now): (afkconvoy_host_reading::spawn::Counter, afkconvoy_host_reading::spawn::Counter),
    wall_ms: u64,
) -> Created {
    let new = |proc: &Proc| now.made_since(before, u64::from(proc.pid));
    Created {
        host: now.since(before),
        alive: procs.iter().filter(|proc| new(proc)).count() as u64,
        mine: counted.iter().filter(|proc| new(proc)).count() as u64,
        wall_ms,
    }
}

/// The commands that created the processes `counted` of a worker found new
/// since `before` — each one's parent, which made it — by how many each
/// made, the most first.
fn spawners(
    procs: &[Proc],
    counted: &[&Proc],
    (before, now): (afkconvoy_host_reading::spawn::Counter, afkconvoy_host_reading::spawn::Counter),
) -> Vec<(String, u64)> {
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|proc| (proc.pid, proc)).collect();
    let mut made: BTreeMap<String, u64> = BTreeMap::new();
    for proc in counted.iter().filter(|proc| now.made_since(before, u64::from(proc.pid))) {
        let parent = by_pid.get(&proc.ppid).map_or_else(|| command_of(proc), |p| command_of(p));
        *made.entry(parent).or_default() += 1;
    }
    let mut made: Vec<(String, u64)> = made.into_iter().collect();
    made.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    made.truncate(5);
    made
}

/// What the last samples found of the commands that created a worker's
/// processes, together: the five that made the most.
fn spawners_of(samples: &VecDeque<Vec<(String, u64)>>) -> Vec<(String, u64)> {
    let mut made: BTreeMap<&str, u64> = BTreeMap::new();
    for (command, count) in samples.iter().flatten() {
        *made.entry(command).or_default() += count;
    }
    let mut made: Vec<(String, u64)> =
        made.into_iter().map(|(command, count)| (command.to_string(), count)).collect();
    made.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    made.truncate(5);
    made
}

/// The processes working in the worktree `worktree` that were handed to
/// pid 1, and every process under one, but those under an exclusive command
/// and servers shared by the host (`shared`, which work outside it): what a
/// command of the worker left behind.
pub(crate) fn orphans_of<'a>(
    procs: &'a [Proc],
    worktree: &Path,
    exclusive: &BTreeSet<u32>,
) -> Vec<&'a Proc> {
    let mut found: Vec<&Proc> = procs
        .iter()
        .filter(|proc| {
            proc.ppid == 1
                && !exclusive.contains(&proc.pid)
                && proc
                    .cwd
                    .as_ref()
                    .is_some_and(|cwd| occupies(std::slice::from_ref(cwd), worktree))
        })
        .collect();
    let mut seen: HashSet<u32> = found.iter().map(|proc| proc.pid).collect();
    let mut next = 0;
    while next < found.len() {
        let pid = found[next].pid;
        next += 1;
        for child in procs.iter().filter(|proc| proc.ppid == pid) {
            if seen.insert(child.pid) {
                found.push(child);
            }
        }
    }
    found
}

/// Ends the orphans of a worker that have outlived the command they came from
/// by `ORPHAN_AFTER`, as `at` finds them: SIGTERM, with SIGCONT so that it is
/// delivered, then SIGKILL at the next sample for one still here. → each one
/// told to end, in words, for its issue.
fn end_orphans(
    watched: &mut Watched,
    procs: &[Proc],
    orphans: &[&Proc],
    at: u64,
) -> Option<Vec<String>> {
    let here: HashSet<u32> = orphans.iter().map(|proc| proc.pid).collect();
    watched.orphans.retain(|pid, _| here.contains(pid));
    let mut ended = vec![];
    for proc in orphans {
        let record = watched.orphans.entry(proc.pid).or_insert(Orphan { since: at, told: None });
        if at.saturating_sub(record.since) < ORPHAN_AFTER {
            continue;
        }
        match record.told {
            None => {
                signal(proc.pid, libc::SIGTERM);
                signal(proc.pid, libc::SIGCONT);
                record.told = Some(at);
                ended.push(format!(
                    "pid {} (under pid 1 for {}s): `{}`",
                    proc.pid,
                    at.saturating_sub(record.since),
                    command_of(proc)
                ));
            }
            Some(_) => signal(proc.pid, libc::SIGKILL),
        }
    }
    let _ = procs;
    (!ended.is_empty()).then_some(ended)
}

fn signal(pid: u32, signal: libc::c_int) {
    // SAFETY: `kill` takes a pid and a signal and touches no memory.
    unsafe { libc::kill(pid as libc::pid_t, signal) };
}

impl Sampler {
    /// Takes one sample of every worker, `every` seconds after the one before
    /// on the run's clock and `wall_ms` on the wall's, at `at`, `queue` the
    /// exclusive queue's tickets and `pressure` the host's memory pressure
    /// level, where it can be read → each turn,
    /// as a line of the log, and what is to be taken down of each worker that
    /// turned hot.
    fn sample(
        &mut self,
        source: &Source,
        at: u64,
        every: u64,
        wall_ms: u64,
        (queue, pressure): (&[Ticket], Option<u64>),
    ) -> Found {
        let measuring = self.samples.is_multiple_of(LISTED_FOR);
        if measuring {
            // One orca could not answer leaves the workers as they were.
            if let Ok(listed) = Worktree::of_workers(source) {
                self.listed = listed
                    .into_iter()
                    .map(|(issue, worktree)| {
                        let worker = match issue {
                            Some(issue) => format!("#{issue}"),
                            None => worktree.path.file_name().map_or_else(String::new, |name| {
                                name.to_string_lossy().into_owned()
                            }),
                        };
                        (worktree.path.clone(), worker, worktree, issue)
                    })
                    .collect();
            }
        }
        self.samples = self.samples.wrapping_add(1);
        let mut turned = vec![];
        // A worker no longer listed is gone: hot, it stops being hot with it.
        let listed: Vec<&PathBuf> = self.listed.iter().map(|(path, ..)| path).collect();
        let gone: Vec<PathBuf> =
            self.watched.keys().filter(|path| !listed.contains(path)).cloned().collect();
        for path in gone {
            let watched = self.watched.remove(&path).expect("listed above");
            if let Some(episode) = watched.watch.hot {
                turned.push(cooled(watched.worker, &episode, true));
            }
        }
        // No worker, nothing to measure: the host's every process is listed,
        // and each one's writes read, only for a worker's tree. A fleet with
        // none listed them every sample, 116 ms of CPU each on a host of
        // 2,150 processes (sunfmin/control#1229).
        let counter =
            if self.listed.is_empty() { None } else { afkconvoy_host_reading::spawn::counter() };
        let procs = if self.listed.is_empty() { vec![] } else { processes() };
        let before_counter = std::mem::replace(&mut self.counter, counter);
        let exclusive = crate::exclusive::running_work(queue, &procs).pids;
        let mut taken = vec![];
        let mut hot_now = vec![];
        let mut ended = vec![];
        let trees: Vec<Vec<&Proc>> =
            self.listed.iter().map(|(path, ..)| tree(&procs, path)).collect();
        let worker_of: HashMap<u32, String> = (self.listed.iter().zip(&trees))
            .flat_map(|((_, worker, ..), tree)| tree.iter().map(|proc| (proc.pid, worker.clone())))
            .collect();
        self.idle.sampled(&trees.iter().flatten().copied().collect::<Vec<_>>(), &procs, at);
        for ((path, worker, worktree, issue), tree) in self.listed.iter().zip(&trees) {
            let watched = self.watched.entry(path.clone()).or_insert_with(|| {
                let mut watched = Watched::new(worker.clone(), worktree.clone(), *issue);
                if let Some(previous) = self.resumed.remove(path) {
                    watched.watch = previous;
                }
                watched
            });
            let idle: Vec<u32> =
                tree.iter().map(|proc| proc.pid).filter(|pid| self.idle.held(*pid, at)).collect();
            watched.stall = stall::of(&idle, &self.idle, &procs, &worker_of, at);
            let orphaned = orphans_of(&procs, path, &exclusive);
            let counted: Vec<&Proc> = {
                let mut counted = tree.clone();
                let seen: HashSet<u32> = counted.iter().map(|proc| proc.pid).collect();
                counted.extend(orphaned.iter().filter(|proc| !seen.contains(&proc.pid)));
                counted
            };
            if let (Some(now), Some(then)) = (counter, before_counter) {
                watched.created.push_back(created(&procs, &counted, (then, now), wall_ms));
                if watched.created.len() > SPAWN_SAMPLES {
                    watched.created.pop_front();
                }
                watched.spawners.push_back(spawners(&procs, &counted, (then, now)));
                if watched.spawners.len() > SPAWN_SAMPLES {
                    watched.spawners.pop_front();
                }
            }
            if let Some(line) = end_orphans(watched, &procs, &orphaned, at) {
                ended.push((watched.worker.clone(), watched.issue, line));
            }
            let took = took(watched, tree, &exclusive);
            watched.resident.push_back((at, took.resident));
            if watched.resident.len() > RESIDENT_KEPT {
                watched.resident.pop_front();
            }
            let held = if measuring { held(path) } else { None };
            taken.push((path.clone(), took, held, watched.stall.is_some()));
        }
        for (path, took, held, stalled) in taken {
            let sample = Sample {
                at,
                millicores: took.cpu_ms.map(|cpu| millicores(cpu, wall_ms)),
                resident: took.resident,
                exclusive: took.exclusive,
                pressure,
                written: took.written,
                spawns: spawn_rate(&self.watched[&path].created),
                held,
                stalled,
            };
            let watched = self.watched.get_mut(&path).expect("watched above");
            match watched.watch.sampled(&sample, &self.limits, every) {
                Some(Turned::Hot(episode)) => {
                    turned.push(hot(watched.worker.clone(), &episode, &self.limits));
                    hot_now
                        .push((watched.worktree.clone(), known(watched, &episode, &self.limits)));
                }
                Some(Turned::Cooled { episode, .. }) => {
                    turned.push(cooled(watched.worker.clone(), &episode, false))
                }
                None => {}
            }
        }
        Found { turned, hot_now, ended }
    }

    /// The workers hot now.
    fn hot(&self) -> Vec<HotNow> {
        let hot = self.watched.values().filter_map(|watched| {
            let episode = watched.watch.hot?;
            Some(HotNow {
                worker: watched.worker.clone(),
                episode,
                path: watched.worktree.path.clone(),
                branch: watched.worktree.branch.clone(),
                issue: watched.issue,
                why: why_said(episode.why, &self.limits),
            })
        });
        hot.collect()
    }
}

/// Thousandths of a core, as a person reads them: `2.5 cores`.
pub fn cores(millicores: u64) -> String {
    format!("{}.{} cores", millicores / 1000, millicores % 1000 / 100)
}

/// Why a worker turned hot, in words: which of what is watched, what it
/// took, and past what.
pub fn why_said(why: Why, limits: &Limits) -> String {
    let past = match why {
        Why::Cpu => format!("past {} for {}s", cores(limits.millicores), CPU_HELD),
        Why::Spawns => {
            format!("past {} processes created a second for {}s", limits.spawns, CPU_HELD)
        }
        Why::Memory => format!("past {} resident", measure::said(limits.resident)),
        Why::Written => format!(
            "past {} written in {} minutes",
            measure::said(limits.written),
            WRITTEN_WITHIN / 60
        ),
        Why::Held => format!(
            "its worktree past {OUTGROWN_TIMES} times its fresh build and past {}",
            measure::said(OUTGROWN_FLOOR)
        ),
        Why::Stalled => format!(
            "a process at no CPU with no children for {STALL_HELD}s, holding build-directory locks no busy process holds"
        ),
    };
    format!("{}: {past}", why.word())
}

/// What an episode took at the most, in words: what its worktree held only
/// once it was measured, and the held pressure readings that counted its
/// memory under `afkconvoy exclusive` only where they did.
pub fn held_said(episode: &Episode) -> String {
    let mut said = format!(
        "{}, {} resident, {} written in {} minutes",
        cores(episode.millicores),
        measure::said(episode.resident),
        measure::said(episode.written),
        WRITTEN_WITHIN / 60
    );
    if episode.held > 0 {
        said += &format!(", {} held in its worktree", measure::said(episode.held));
    }
    if let Some(pressed) = episode.pressed {
        let levels = if pressed.lowest == pressed.highest {
            format!("level {}", pressed.lowest)
        } else {
            format!("levels {} to {}", pressed.lowest, pressed.highest)
        };
        said += &format!(
            ", its memory under exclusive counted for memory pressure at {levels} in {} readings over {}s",
            pressed.readings,
            pressed.at.saturating_sub(pressed.since)
        );
    }
    said
}

fn hot(worker: String, episode: &Episode, limits: &Limits) -> What {
    What::Hot { worker, why: why_said(episode.why, limits), held: held_said(episode) }
}

fn cooled(worker: String, episode: &Episode, gone: bool) -> What {
    What::Cooled { worker, since: episode.since, held: held_said(episode), gone }
}

/// Names a stalled worker's stall, with the holders of the locks it waits
/// on, on its issue (docs/adr/0088): nothing else ends a deadlock, and its
/// worker cannot see it. A worker turned hot for anything else, or linked to
/// no issue, is named nowhere.
fn name_stall(repo: &Repo, taken: &Taken) -> anyhow::Result<()> {
    let (Some(stall), Some(issue)) = (&taken.stall, taken.issue) else { return Ok(()) };
    let body = format!(
        "**afkconvoy** — this worker's build is stalled: {stall}.\n\n\
         Nothing releases these locks: end the stalled processes (SIGTERM) so their \
         builds run again (docs/adr/0088)."
    );
    crate::gh::GitHub::of(repo)?.comment(issue, &body)
}

/// Names the process creation rate of a worker that turned hot for it, and
/// the commands that made the processes, on its issue (sunfmin/control#1319):
/// what it costs is paid in the kernel and in daemons outside its tree, and
/// nothing else shows who spends it. A worker turned hot for anything else,
/// or linked to no issue, is named nowhere.
fn name_spawns(repo: &Repo, taken: &Taken) -> anyhow::Result<()> {
    let (Some((rate, commands)), Some(issue)) = (&taken.spawns, taken.issue) else {
        return Ok(());
    };
    let mut body = format!(
        "**afkconvoy** — this worker's processes create about {rate} processes a second, held \
         for {CPU_HELD}s: process creation is paid in the kernel and in the system daemons each \
         exec wakes, and every command waits on the exclusive queue while it lasts \
         (sunfmin/control#1319).\n\nThe commands that created them:\n"
    );
    if commands.is_empty() {
        body.push_str("\n- (none found alive in the last two minutes of samples)");
    }
    for (command, made) in commands {
        body += &format!("\n- `{command}`: {made} found alive in the last two minutes of samples");
    }
    crate::gh::GitHub::of(repo)?.comment(issue, &body)
}

/// Names the processes a worker's commands left behind and that were told to
/// end, on its issue.
fn name_orphans(repo: &Repo, issue: Option<u64>, ended: &[String]) -> anyhow::Result<()> {
    let Some(issue) = issue else { return Ok(()) };
    let mut body = format!(
        "**afkconvoy** — processes a command of this worker left behind, handed to pid 1 and \
         still working in its worktree after {ORPHAN_AFTER}s, were told to end (SIGTERM, then \
         SIGKILL at the next sample):\n"
    );
    for one in ended {
        body += &format!("\n- {one}");
    }
    crate::gh::GitHub::of(repo)?.comment(issue, &body)
}

/// Who hears what a watcher finds (`watch`): its fleet instance.
pub trait Hears: Send + Sync {
    /// A worker turned hot, or stopped being hot, at `at`.
    fn turned(&self, at: u64, what: What);
    /// Something went wrong that costs the watcher nothing but what it was
    /// doing.
    fn warn(&self, warning: &str);
}

/// Watches every worker of the checkout `repo` is of, on a thread of its own,
/// until `stop` is told: a sample every `SAMPLE_EVERY` seconds of `second`
/// each, timed by `clock` — the run's — and `hears` told of each turn
/// (`Sampler::sample`), under the limits the host settings say as it takes
/// it. `hot` is kept as the workers hot now. What shows a worker that turned
/// hot is taken down on a thread of its own, and the fleet instance woken by
/// `waker` once it is. A second that takes no time samples nothing.
pub fn watch(
    repo: &Repo,
    second: Duration,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    hears: Arc<dyn Hears>,
    (hot, resources, queue): (Hot, crate::resources::Shared, crate::exclusive::Queue),
    waker: Waker,
    stop: Stop,
) {
    if second.is_zero() {
        return;
    }
    let repo = Repo::new(repo.dir.clone());
    let every = second * SAMPLE_EVERY as u32;
    let said = host_settings::of_host().map(|settings| settings.hot()).unwrap_or_default();
    let mut sampler = Sampler {
        said,
        limits: limits(said, &repo.dir),
        listed: vec![],
        samples: 0,
        watched: BTreeMap::new(),
        resumed: crate::resources::samples(
            &repo.main_checkout().unwrap_or_else(|_| repo.dir.clone()),
        )
        .unwrap_or_default(),
        idle: Idle::default(),
        counter: None,
    };
    std::thread::spawn(move || {
        let started = (clock(), Instant::now());
        let mut source = None;
        let mut before = Instant::now();
        loop {
            std::thread::sleep(every);
            // The fleet instance it watches for ended, and a start of it
            // again watches for itself.
            if stop.told() {
                return;
            }
            if source.is_none() {
                source = Source::of(&repo).ok();
            }
            let Some(source) = &source else { continue };
            // The run's clock, at the pace its seconds take: a test's are short.
            let elapsed = started.1.elapsed().as_millis() as u64;
            let at = started.0 + elapsed / (second.as_millis() as u64).max(1);
            let wall_ms = before.elapsed().as_millis() as u64;
            before = Instant::now();
            // Settings that cannot be read leave the limits as they were.
            if let Ok(settings) = host_settings::of_host()
                && settings.hot() != sampler.said
            {
                sampler.said = settings.hot();
                sampler.limits = limits(sampler.said, &repo.dir);
            }
            // A queue that cannot be read leaves every process counted.
            let tickets = queue.observation().map_or_else(|_| vec![], |(tickets, _)| tickets);
            let pressure = crate::resources::pressure_level();
            let Found { turned, hot_now: to_take, ended } =
                sampler.sample(source, at, SAMPLE_EVERY, wall_ms, (&tickets, pressure));
            for (worker, issue, ended) in ended {
                hears.turned(at, What::Orphans { worker: worker.clone(), ended: ended.clone() });
                let (waker, hears) = (waker.clone(), Arc::clone(&hears));
                let repo = Repo::new(repo.dir.clone());
                std::thread::spawn(move || {
                    if let Err(error) = name_orphans(&repo, issue, &ended) {
                        hears.warn(&format!(
                            "{worker}'s orphans were not named on its issue: {error:#}"
                        ));
                    }
                    waker.wake();
                });
            }
            for turned in turned {
                hears.turned(at, turned);
            }
            for (worktree, taken) in to_take {
                let (waker, hears) = (waker.clone(), Arc::clone(&hears));
                let repo = Repo::new(repo.dir.clone());
                std::thread::spawn(move || {
                    if let Err(error) = evidence::take(&worktree, &taken) {
                        let worker = &taken.worker;
                        hears.warn(&format!(
                            "what shows {worker} hot was not taken down: {error:#}"
                        ));
                    }
                    if let Err(error) = name_spawns(&repo, &taken) {
                        let worker = &taken.worker;
                        hears.warn(&format!(
                            "{worker}'s process creation was not named on its issue: {error:#}"
                        ));
                    }
                    if let Err(error) = name_stall(&repo, &taken) {
                        let worker = &taken.worker;
                        hears.warn(&format!(
                            "{worker}'s stall was not named on its issue: {error:#}"
                        ));
                    }
                    waker.wake();
                });
            }
            let current = sampler.hot();
            let samples = sampler
                .watched
                .iter()
                .map(|(path, worker)| (path.clone(), worker.watch.clone()))
                .collect();
            if let Err(error) = resources.hot(
                repo.main_checkout().unwrap_or_else(|_| repo.dir.clone()),
                current.clone(),
                samples,
            ) {
                hears.warn(&format!("resource episodes were not kept: {error:#}"));
            }
            *hot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_is_read_as_ps_says_it() {
        assert_eq!(cpu_time("0:00.01"), Some(10));
        assert_eq!(cpu_time("712:03.95"), Some((712 * 60 + 3) * 1000 + 950));
        assert_eq!(cpu_time("1:02:03.4"), Some(3_723_400));
        assert_eq!(cpu_time("2-00:00:01"), Some((2 * 86_400 + 1) * 1000));
        assert_eq!(cpu_time("nonsense"), None);
    }

    #[test]
    fn a_row_of_ps_is_its_numbers_then_its_line() {
        let row =
            "  630 94216  12.3   3024   0:01.50 /bin/zsh -c x TMPDIR=/w/.afkconvoy-tmp HOME=/u";
        let proc = parse(row).unwrap();
        assert_eq!(
            (proc.pid, proc.ppid, proc.cpu_share, proc.resident, proc.cpu_ms),
            (630, 94216, 1230, 3024 * 1024, 1500)
        );
        assert_eq!(parse("1 0 nan 8 0:00.00 launchd").map(|proc| proc.cpu_share), Some(0));
        assert_eq!(proc.line, "/bin/zsh -c x TMPDIR=/w/.afkconvoy-tmp HOME=/u");
        assert_eq!(parse("not a row"), None);
    }

    fn proc(pid: u32, ppid: u32, line: &str) -> Proc {
        Proc {
            pid,
            ppid,
            resident: 1024,
            cpu_ms: 0,
            cpu_share: 0,
            written: 0,
            line: line.to_string(),
            cwd: None,
        }
    }

    /// A process handed to pid 1, started with `line`, working in `cwd`.
    fn orphan(pid: u32, line: &str, cwd: &str) -> Proc {
        Proc { cwd: Some(PathBuf::from(cwd)), ..proc(pid, 1, line) }
    }

    fn pids(procs: &[Proc], worktree: &str) -> Vec<u32> {
        let mut pids: Vec<u32> = tree(procs, Path::new(worktree)).iter().map(|p| p.pid).collect();
        pids.sort();
        pids
    }

    /// The `TMPDIR` of the worker in `worktree`.
    fn tmp(worktree: &str) -> String {
        worker_tmp::of(Path::new(worktree)).display().to_string()
    }

    #[test]
    fn a_workers_processes_are_those_started_with_its_tmpdir_and_all_under_them() {
        let (a, b) = (tmp("/w/a"), tmp("/w/b"));
        let procs = [
            proc(1, 0, "launchd"),
            proc(10, 1, "zsh -l"),
            proc(11, 10, &format!("claude TMPDIR={a} HOME=/u")),
            proc(12, 11, "cargo test"),
            proc(13, 12, "rustc"),
            proc(20, 1, &format!("claude TMPDIR={a}-2 HOME=/u")),
            proc(21, 1, &format!("claude TMPDIR={b}")),
            proc(30, 1, &format!("cargo TMPDIR={a}/")),
        ];
        assert_eq!(pids(&procs, "/w/a"), [11, 12, 13, 30]);
        assert_eq!(pids(&procs, "/w/b"), [21]);
        assert!(pids(&procs, "/w/c").is_empty());
    }

    #[test]
    fn a_server_a_worker_started_for_the_host_is_no_workers() {
        let (a, b) = (tmp("/w/a"), tmp("/w/b"));
        let procs = [
            proc(11, 10, &format!("claude TMPDIR={a}")),
            proc(12, 11, &format!("cargo build TMPDIR={a}")),
            proc(13, 12, &format!("sccache rustc TMPDIR={a}")),
            // the server a's first build started, which works in `/`…
            orphan(40, &format!("sccache SCCACHE_START_SERVER=1 TMPDIR={a}"), "/"),
            // …compiling for b, and for a
            proc(41, 40, &format!("rustc --crate-name b TMPDIR={b}")),
            proc(42, 40, &format!("rustc --crate-name a TMPDIR={a}")),
            proc(43, 42, "ld"),
            proc(21, 20, &format!("claude TMPDIR={b}")),
        ];
        assert_eq!(pids(&procs, "/w/a"), [11, 12, 13, 42, 43]);
        assert_eq!(pids(&procs, "/w/b"), [21, 41]);
    }

    #[test]
    fn a_process_a_worker_left_behind_is_still_its_own() {
        let a = tmp("/w/a");
        let procs = [
            // a test its runner left, still in the worktree, and what it runs
            orphan(50, &format!("deps/cli-0123 a_test TMPDIR={a}"), "/w/a/crates/cli"),
            proc(51, 50, &format!("afkconvoy run TMPDIR={a}")),
            // one whose directory cannot be read
            proc(60, 1, &format!("cargo TMPDIR={a}")),
        ];
        assert_eq!(pids(&procs, "/w/a"), [50, 51, 60]);
    }

    #[test]
    fn the_fleets_processes_are_those_started_in_a_worktree_it_put_a_worker_in() {
        let link = worker_tmp::of(Path::new("/w/b")).display().to_string();
        let procs = [
            proc(1, 0, "launchd"),
            proc(10, 1, "zsh -l"),
            proc(11, 10, "claude TMPDIR=/w/a/.afkconvoy-tmp HOME=/u"),
            proc(12, 11, "cargo test"),
            // A gate there sets a TMPDIR of its own under the worktree's,
            // and its tests, left behind as their gate was killed, are still
            // the fleet's.
            proc(13, 1, "nextest TMPDIR=/w/a/.afkconvoy-tmp/the-tests HOME=/u"),
            // A human's gate in their own checkout, the user's shell, orca.
            proc(20, 1, "nextest TMPDIR=/src/widgets/.afkconvoy-tmp/the-tests HOME=/u"),
            proc(21, 1, "zsh TMPDIR=/var/folders/x HOME=/u"),
            proc(22, 1, "/Applications/Orca.app/Contents/MacOS/Orca"),
            proc(23, 1, "claude TMPDIR=/w/a/.afkconvoy-tmp-2"),
            // A worker started with the link the fleet made for its
            // worktree, and a server it left, under the link, its worktree
            // gone.
            proc(30, 10, &format!("claude TMPDIR={link} HOME=/u")),
            proc(31, 1, &format!("sccache TMPDIR={link}/sccache HOME=/u")),
            proc(32, 1, "sh TMPDIR=/tmp/afkconvoy-0x/abc HOME=/u"),
        ];
        let ours = |worktree: &Path| worktree.starts_with("/w");
        let mut pids: Vec<u32> = fleets(&procs, ours).into_iter().collect();
        pids.sort();
        assert_eq!(pids, [11, 12, 13, 30, 31]);
        assert_eq!(worktree_of("/w/a/.afkconvoy-tmp"), Some(PathBuf::from("/w/a")));
        assert_eq!(worktree_of("/.afkconvoy-tmp"), None);
        assert_eq!(worktree_of("/w/a/.afkconvoy-tmp-2/x"), None);
    }

    #[test]
    fn what_runs_under_an_exclusive_command_is_its_running_tickets_process_and_all_under_it() {
        // a worker's harness 10, its gate 11, the gate's `afkconvoy exclusive`
        // 12 running cargo 13 and rustc 14; another ticket's 20, waiting
        let procs = [
            proc(10, 1, "claude"),
            proc(11, 10, "afkconvoy gate"),
            proc(12, 11, "afkconvoy exclusive -- ./gate.sh"),
            proc(13, 12, "cargo nextest run"),
            proc(14, 13, "rustc"),
            proc(20, 10, "afkconvoy exclusive -- make"),
        ];
        let ticket = |number: u64, pid: u32, ran: Option<u64>| Ticket {
            number,
            pid,
            command: String::new(),
            dir: "/w/a".into(),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            whole_repo: false,
            note: afkconvoy_core::exclusive::Note { issue: None, validation: None },
            gate: None,
            asked: 0,
            ran,
            retained: None,
        };
        let queue = [ticket(1, 12, Some(0)), ticket(2, 20, None)];
        let under = crate::exclusive::running_work(&queue, &procs).pids;
        assert_eq!(under.into_iter().collect::<Vec<_>>(), [12, 13, 14]);
        assert!(crate::exclusive::running_work(&queue[1..], &procs).pids.is_empty());
    }

    #[test]
    fn what_a_worker_took_is_what_its_processes_took_since_the_sample_before() {
        let worktree = Worktree::at(PathBuf::from("/w/a"), "a".into());
        let mut watched = Watched::new("#7".into(), worktree, Some(7));
        let at = |taken: &[(u32, u64, u64)]| -> Vec<Proc> {
            let one = |(pid, ms, written): &(u32, u64, u64)| Proc {
                cpu_ms: *ms,
                written: *written,
                ..proc(*pid, 1, "")
            };
            taken.iter().map(one).collect()
        };
        let first = at(&[(1, 5000, 1 << 20), (2, 700, 0)]);
        assert_eq!(
            took(&mut watched, &first.iter().collect::<Vec<_>>(), &BTreeSet::from([2])),
            Took { cpu_ms: None, written: None, resident: 2048, exclusive: 1024 }
        );
        // 1 took 2000 more and wrote 1 KiB more, 2 ended, 3 started and took
        // 300, writing 4 KiB
        let second = at(&[(1, 7000, (1 << 20) + 1024), (3, 300, 4096)]);
        assert_eq!(
            took(&mut watched, &second.iter().collect::<Vec<_>>(), &BTreeSet::new()),
            Took { cpu_ms: Some(2300), written: Some(5120), resident: 2048, exclusive: 0 }
        );
        // 1 took the most since the sample before
        assert_eq!((watched.hottest, watched.pids.clone()), (Some(1), vec![1, 3]));
    }

    #[test]
    fn what_a_hot_worker_took_is_said_by_which_of_what_is_watched() {
        assert_eq!(cores(2500), "2.5 cores");
        let limits = Limits { millicores: 4000, resident: 8 << 30, written: 20 << 30, spawns: 300 };
        assert_eq!(why_said(Why::Cpu, &limits), "CPU: past 4.0 cores for 120s");
        assert_eq!(why_said(Why::Memory, &limits), "memory: past 8.0 GiB resident");
        assert_eq!(
            why_said(Why::Written, &limits),
            "disk written: past 20.0 GiB written in 10 minutes"
        );
        assert_eq!(
            why_said(Why::Held, &limits),
            "disk held: its worktree past 3 times its fresh build and past 1.0 GiB"
        );
        let episode = Episode {
            since: 0,
            why: Why::Cpu,
            millicores: 6200,
            spawns: 0,
            resident: 3 << 29,
            written: 300 << 20,
            held: 0,
            pressed: None,
        };
        assert_eq!(
            held_said(&episode),
            "6.2 cores, 1.5 GiB resident, 300.0 MiB written in 10 minutes"
        );
        let held = Episode { held: 5 << 30, ..episode };
        assert!(
            held_said(&held).ends_with(", 5.0 GiB held in its worktree"),
            "{}",
            held_said(&held)
        );
        let pressed = afkconvoy_core::hot::Pressed {
            since: 990,
            at: 1110,
            readings: 12,
            lowest: 2,
            highest: 3,
        };
        let counted = Episode { why: Why::Memory, pressed: Some(pressed), ..episode };
        assert!(
            held_said(&counted).ends_with(
                ", its memory under exclusive counted for memory pressure at levels 2 to 3 in 12 readings over 120s"
            ),
            "{}",
            held_said(&counted)
        );
        let one = Episode {
            pressed: Some(afkconvoy_core::hot::Pressed { highest: 2, ..pressed }),
            ..counted
        };
        assert!(held_said(&one).contains("pressure at level 2 in 12"), "{}", held_said(&one));
    }

    #[test]
    fn a_worktree_is_measured_only_once_its_fresh_build_is_on_record() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = crate::process::output(
                crate::census::command("git").args(args).current_dir(dir.path()),
            )
            .unwrap();
            assert!(ok.status.success(), "{ok:?}");
        };
        git(&["init", "--quiet"]);
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "x"]);
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/built"), vec![0; 3000]).unwrap();
        assert_eq!(held(dir.path()), None, "no fresh build on record");
        measure::record_fresh(&Repo::new(dir.path().to_path_buf()), 1000).unwrap();
        assert_eq!(held(dir.path()), Some(Built { built: 3000, fresh: Some(1000) }));
    }

    /// A worktree of the tests' own, with a `TMPDIR` of its own.
    fn world() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().canonicalize().unwrap();
        let tmp = worker_tmp::of(&worktree);
        std::fs::create_dir_all(&tmp).unwrap();
        let tmp = tmp.display().to_string();
        (dir, worktree, tmp)
    }

    /// The loop of the fixture below, run as a process of its own: `sleep
    /// 0.01`, over and over, for a minute at the most.
    #[test]
    #[ignore = "a fixture of another test, run as a process of its own"]
    fn fixture_spinning_sleeps() {
        let until = Instant::now() + Duration::from_secs(60);
        while Instant::now() < until {
            let _ = crate::census::command("sleep").arg("0.01").status();
        }
    }

    /// A fixture spinning `sleep 0.01` under a worker creates processes at
    /// a rate that turns it hot (sunfmin/control#1319).
    #[test]
    fn a_worker_spinning_sleeps_is_hot_by_its_rate_of_process_creation() {
        use afkconvoy_host_reading::spawn::counter;
        let (_dir, worktree, tmp) = world();
        // A system binary's environment is not readable (SIP): the worker's
        // own process, this test binary running the loop below, is.
        /// Ends the fixture however the test ends: a spinner left behind is
        /// the very thing this is about.
        struct Spinner(std::process::Child);
        impl Drop for Spinner {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let spinner = crate::census::command(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "hot::tests::fixture_spinning_sleeps"])
            .current_dir(&worktree)
            .env("TMPDIR", &tmp)
            .spawn()
            .unwrap();
        let spinner = Spinner(spinner);
        let mut watched =
            Watched::new("#1".into(), Worktree::at(worktree.clone(), "b".into()), None);
        let mut before = counter().unwrap();
        let started = Instant::now();
        let mut spinner_says = String::new();
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            let now = counter().unwrap();
            let procs = listed();
            let mine = tree(&procs, &worktree);
            let found = mine.iter().find(|proc| proc.pid == spinner.0.id());
            spinner_says = command_of(found.unwrap_or_else(|| panic!("{mine:?}")));
            watched.created.push_back(created(&procs, &mine, (before, now), 100));
            watched.spawners.push_back(spawners(&procs, &mine, (before, now)));
            before = now;
        }
        let rate = spawn_rate(&watched.created).expect("processes were created and found");
        let seconds = started.elapsed().as_secs_f64();
        drop(spinner);
        assert!(rate >= 15, "{rate} processes a second over {seconds}s");
        assert!(
            spawners_of(&watched.spawners).iter().any(|(command, _)| *command == spinner_says),
            "{:?}",
            watched.spawners
        );
        // held for two minutes, past the bound, it is hot by it
        let limits = Limits { millicores: 4000, resident: 8 << 30, written: 20 << 30, spawns: 5 };
        let mut turned = None;
        for at in (1000..=1000 + CPU_HELD + SAMPLE_EVERY).step_by(SAMPLE_EVERY as usize) {
            let sample = Sample {
                at,
                millicores: Some(0),
                resident: 0,
                exclusive: 0,
                pressure: None,
                written: Some(0),
                spawns: Some(rate),
                held: None,
                stalled: false,
            };
            turned = turned.or(watched.watch.sampled(&sample, &limits, SAMPLE_EVERY));
        }
        let Some(Turned::Hot(episode)) = turned else { panic!("{turned:?}") };
        assert_eq!(episode.why, Why::Spawns);
        assert_eq!(
            why_said(Why::Spawns, &limits),
            "process creation: past 5 processes created a second for 120s"
        );
    }

    /// A process a command of the worker left behind, under pid 1 in its
    /// worktree, is named once it has outlived the command by the bound, and
    /// ended.
    #[test]
    fn an_orphan_outliving_its_command_is_named_and_ended() {
        let (_dir, worktree, tmp) = world();
        // The command: its background `sleep` outlives it, handed to pid 1.
        let status = crate::census::command("sh")
            .args(["-c", "(sleep 600 &) ; true"])
            .current_dir(&worktree)
            .env("TMPDIR", &tmp)
            .status()
            .unwrap();
        assert!(status.success());
        std::thread::sleep(Duration::from_millis(300));
        let mut watched =
            Watched::new("#1".into(), Worktree::at(worktree.clone(), "b".into()), None);
        let procs = listed();
        let orphans = orphans_of(&procs, &worktree, &BTreeSet::new());
        assert_eq!(orphans.len(), 1, "{orphans:?}");
        let pid = orphans[0].pid;
        // first seen: nothing to end yet
        assert_eq!(end_orphans(&mut watched, &procs, &orphans, 1000), None);
        assert_eq!(end_orphans(&mut watched, &procs, &orphans, 1000 + ORPHAN_AFTER - 10), None);
        let ended = end_orphans(&mut watched, &procs, &orphans, 1000 + ORPHAN_AFTER)
            .expect("outlived the command by the bound");
        assert_eq!(ended.len(), 1, "{ended:?}");
        assert!(
            ended[0].contains(&format!("pid {pid}")) && ended[0].contains("sleep 600"),
            "{ended:?}"
        );
        // it is gone, as SIGTERM ends a sleep
        let mut gone = false;
        for _ in 0..50 {
            if !listed().iter().any(|proc| proc.pid == pid) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(gone, "pid {pid} still runs");
    }

    /// A server shared by the host — handed to pid 1, working outside the
    /// worktree — is no orphan of the worker's, nor one under an exclusive
    /// command.
    #[test]
    fn a_shared_server_and_an_exclusive_command_are_no_orphans() {
        let worktree = PathBuf::from("/w/a");
        let procs = [
            orphan(20, "sccache --start-server", "/"),
            orphan(21, "sleep 600", "/w/a/target"),
            orphan(22, "cargo test", "/w/a"),
            proc(23, 21, "sleep 1"),
        ];
        let found: Vec<u32> = orphans_of(&procs, &worktree, &BTreeSet::from([22]))
            .iter()
            .map(|proc| proc.pid)
            .collect();
        assert_eq!(found, [21, 23]);
    }
}

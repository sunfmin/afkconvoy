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
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use afkconvoy_core::exclusive::Ticket;
use afkconvoy_core::hot::{
    CPU_HELD, Episode, Limits, SAMPLE_EVERY, Sample, Turned, WRITTEN_WITHIN, Watch, Why, millicores,
};
use afkconvoy_core::landing::{Built, OUTGROWN_FLOOR, OUTGROWN_TIMES};
use serde::{Deserialize, Serialize};

use crate::evidence::{self, Taken};
use crate::git::Repo;
use crate::host_settings;
use crate::log::What;
use crate::measure;
use crate::orca::{Source, Worktree};
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
/// `--hot-cores`, `--hot-memory` and `--hot-written`, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Said {
    pub millicores: Option<u64>,
    pub resident: Option<u64>,
    pub written: Option<u64>,
}

/// The limits on this host for the fleet of the checkout `dir`: its shares
/// (`Limits::of_host`) — of the disk `dir` is on — but for what it says.
pub fn limits(said: Said, dir: &Path) -> Limits {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get() as u64);
    limits_from(said, cores, memory(), free_disk(dir))
}

/// The limits derived from an existing host reading.
pub fn limits_from(said: Said, cores: u64, memory: Option<u64>, disk: Option<u64>) -> Limits {
    let shares = Limits::of_host(cores, memory.unwrap_or(u64::MAX), disk.unwrap_or(u64::MAX));
    Limits {
        millicores: said.millicores.unwrap_or(shares.millicores),
        resident: said.resident.unwrap_or(shares.resident),
        written: said.written.unwrap_or(shares.written),
    }
}

/// The bytes free on the disk `dir` is on, as `df` says them; `None` where
/// they cannot be read.
fn free_disk(dir: &Path) -> Option<u64> {
    let out = crate::process::output(Command::new("df").arg("-Pk").arg(dir)).ok()?;
    let out = String::from_utf8_lossy(&out.stdout).into_owned();
    let kib: u64 = out.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kib.saturating_mul(1024))
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
    let mut dirs = vec![];
    for proc in listed_checked()? {
        if let Some(cwd) = works_in(proc.pid) {
            dirs.push(cwd);
        }
    }
    Ok(dirs)
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
    let out = crate::process::output(Command::new("sysctl").args(["-n", name])).ok()?;
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
    let out = crate::process::output(Command::new("ps").args([
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
fn shared(proc: &Proc, worktree: &Path) -> bool {
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
    ) -> (Vec<What>, Vec<(Worktree, Taken)>) {
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
        let procs = processes();
        let exclusive = crate::exclusive::running_work(queue, &procs).pids;
        let mut taken = vec![];
        let mut hot_now = vec![];
        for (path, worker, worktree, issue) in &self.listed {
            let watched = self.watched.entry(path.clone()).or_insert_with(|| {
                let mut watched = Watched::new(worker.clone(), worktree.clone(), *issue);
                if let Some(previous) = self.resumed.remove(path) {
                    watched.watch = previous;
                }
                watched
            });
            let took = took(watched, &tree(&procs, path), &exclusive);
            watched.resident.push_back((at, took.resident));
            if watched.resident.len() > RESIDENT_KEPT {
                watched.resident.pop_front();
            }
            let held = if measuring { held(path) } else { None };
            taken.push((path.clone(), took, held));
        }
        for (path, took, held) in taken {
            let sample = Sample {
                at,
                millicores: took.cpu_ms.map(|cpu| millicores(cpu, wall_ms)),
                resident: took.resident,
                exclusive: took.exclusive,
                pressure,
                written: took.written,
                held,
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
        (turned, hot_now)
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
            let (turned, to_take) =
                sampler.sample(source, at, SAMPLE_EVERY, wall_ms, (&tickets, pressure));
            for turned in turned {
                hears.turned(at, turned);
            }
            for (worktree, taken) in to_take {
                let (waker, hears) = (waker.clone(), Arc::clone(&hears));
                std::thread::spawn(move || {
                    if let Err(error) = evidence::take(&worktree, &taken) {
                        let worker = &taken.worker;
                        hears.warn(&format!(
                            "what shows {worker} hot was not taken down: {error:#}"
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
            note: afkconvoy_core::exclusive::Note { issue: None, validation: None },
            gate: None,
            asked: 0,
            ran,
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
        let limits = Limits { millicores: 4000, resident: 8 << 30, written: 20 << 30 };
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
            let ok = crate::process::output(Command::new("git").args(args).current_dir(dir.path()))
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
}

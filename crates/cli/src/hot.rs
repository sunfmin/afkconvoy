//! Hot workers, as `run` finds them (docs/adr/0019): every `SAMPLE_EVERY`
//! seconds it reads what each worker's processes take of the CPU and memory —
//! from each process's own usage, as `ps` says it, with no root — and the
//! episode rule (`afkconvoy_core::hot`) says which turned hot and which
//! stopped being hot. Each turn is a line of the log.
//!
//! A worker's processes are those started with its `TMPDIR`, the one of its
//! worktree, and every process under one of them: a process whose
//! environment cannot be read — a system binary — is counted through the one
//! it was started under. The landing train's worktree is no worker's.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use afkconvoy_core::hot::{
    CPU_HELD, Episode, Limits, SAMPLE_EVERY, Sample, Turned, Watch, Why, millicores,
};
use serde::Serialize;

use crate::git::Repo;
use crate::log::What;
use crate::measure;
use crate::orca::{Source, Worktree};

/// How many samples go by before the worktrees are asked of orca again.
const LISTED_FOR: u32 = 6;

/// What the host says of the limits, where it says any: `--hot-cores` and
/// `--hot-memory`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Said {
    pub millicores: Option<u64>,
    pub resident: Option<u64>,
}

/// The limits on this host: its shares (`Limits::of_host`), but for what it
/// says.
pub fn limits(said: Said) -> Limits {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get() as u64);
    let shares = Limits::of_host(cores, memory().unwrap_or(u64::MAX));
    Limits {
        millicores: said.millicores.unwrap_or(shares.millicores),
        resident: said.resident.unwrap_or(shares.resident),
    }
}

/// The bytes of memory this host has; `None` where it cannot be read.
fn memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        return sysctl("hw.memsize");
    }
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib.saturating_mul(1024))
}

/// Whether this host is under memory pressure: warned or worse, as macOS
/// says it. A host that does not say is under none.
fn under_pressure() -> bool {
    cfg!(target_os = "macos")
        && sysctl("kern.memorystatus_vm_pressure_level").is_some_and(|level| level >= 2)
}

fn sysctl(name: &str) -> Option<u64> {
    let out = Command::new("sysctl").args(["-n", name]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// A worker that is hot now, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotNow {
    /// `#7`, or the name of its worktree where it is linked to no issue.
    pub worker: String,
    pub episode: Episode,
}

/// The workers that are hot now, shared between the sampler and the panel.
pub type Hot = Arc<Mutex<Vec<HotNow>>>;

/// One process, as `ps` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Proc {
    pid: u32,
    ppid: u32,
    /// Resident memory, in bytes.
    resident: u64,
    /// The CPU it has taken since it started, in milliseconds.
    cpu_ms: u64,
    /// Its command line, then its environment, where that can be read.
    line: String,
}

/// Every process of this host, as `ps` says them.
fn processes() -> Vec<Proc> {
    let out = Command::new("ps").args(["axeww", "-o", "pid=,ppid=,rss=,time=,command="]).output();
    out.map(|out| String::from_utf8_lossy(&out.stdout).lines().filter_map(parse).collect())
        .unwrap_or_default()
}

/// One row of `ps`: pid, ppid, resident KiB, CPU time, then the rest.
fn parse(row: &str) -> Option<Proc> {
    let mut rest = row.trim_start();
    let mut field = || {
        let (word, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        rest = after.trim_start();
        word
    };
    let pid = field().parse().ok()?;
    let ppid = field().parse().ok()?;
    let resident = field().parse::<u64>().ok()?.saturating_mul(1024);
    let cpu_ms = cpu_time(field())?;
    Some(Proc { pid, ppid, resident, cpu_ms, line: rest.to_string() })
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

/// The processes of the worker started with `tmp` for its `TMPDIR`: each
/// started with it, and each under one that was.
fn tree<'a>(procs: &'a [Proc], tmp: &str) -> Vec<&'a Proc> {
    let needle = format!(" TMPDIR={tmp}");
    let started_with = |proc: &Proc| {
        proc.line.match_indices(&needle).any(|(at, _)| {
            let after = &proc.line[at + needle.len()..];
            let after = after.strip_prefix('/').unwrap_or(after);
            after.is_empty() || after.starts_with(' ')
        })
    };
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for proc in procs {
        children.entry(proc.ppid).or_default().push(proc);
    }
    let mut found: Vec<&Proc> = procs.iter().filter(|proc| started_with(proc)).collect();
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

/// One worker the sampler watches.
struct Watched {
    worker: String,
    watch: Watch,
    /// The CPU each of its processes had taken at the sample before; `None`
    /// before its first.
    cpu: Option<HashMap<u32, u64>>,
}

/// What a worker's processes took in one sample: the CPU since the sample
/// before, in milliseconds — `None` at its first — and the bytes resident.
fn took(watched: &mut Watched, procs: &[&Proc]) -> (Option<u64>, u64) {
    let now: HashMap<u32, u64> = procs.iter().map(|proc| (proc.pid, proc.cpu_ms)).collect();
    // A process new since the sample before took all it has since then.
    let cpu = watched.cpu.as_ref().map(|before| {
        let since = |(pid, ms): (&u32, &u64)| ms.saturating_sub(*before.get(pid).unwrap_or(&0));
        now.iter().map(since).sum()
    });
    watched.cpu = Some(now);
    (cpu, procs.iter().map(|proc| proc.resident).sum())
}

/// What the sampler keeps between samples.
struct Sampler {
    limits: Limits,
    /// The worktrees of the workers, by path, as orca last listed them.
    listed: Vec<(PathBuf, String)>,
    samples: u32,
    watched: BTreeMap<PathBuf, Watched>,
}

impl Sampler {
    /// Takes one sample of every worker, `every` seconds after the one before
    /// on the run's clock and `wall_ms` on the wall's, at `at` → each turn,
    /// as a line of the log.
    fn sample(&mut self, source: &Source, at: u64, every: u64, wall_ms: u64) -> Vec<What> {
        if self.samples.is_multiple_of(LISTED_FOR) {
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
                        (worktree.tmp_dir(), worker)
                    })
                    .collect();
            }
        }
        self.samples = self.samples.wrapping_add(1);
        let mut turned = vec![];
        // A worker no longer listed is gone: hot, it stops being hot with it.
        let listed: Vec<&PathBuf> = self.listed.iter().map(|(tmp, _)| tmp).collect();
        let gone: Vec<PathBuf> =
            self.watched.keys().filter(|tmp| !listed.contains(tmp)).cloned().collect();
        for tmp in gone {
            let watched = self.watched.remove(&tmp).expect("listed above");
            if let Some(episode) = watched.watch.hot {
                turned.push(cooled(watched.worker, &episode, true));
            }
        }
        let procs = processes();
        let mut taken = vec![];
        for (tmp, worker) in &self.listed {
            let watched = self.watched.entry(tmp.clone()).or_insert_with(|| Watched {
                worker: worker.clone(),
                watch: Watch::default(),
                cpu: None,
            });
            let (cpu, resident) = took(watched, &tree(&procs, &tmp.display().to_string()));
            taken.push((tmp.clone(), cpu.map(|cpu| millicores(cpu, wall_ms)), resident));
        }
        let pressure = under_pressure();
        let most = taken
            .iter()
            .filter(|(_, _, resident)| *resident > 0)
            .max_by_key(|(_, _, resident)| *resident);
        let most = most.map(|(tmp, _, _)| tmp.clone());
        for (tmp, millicores, resident) in taken {
            let most_under_pressure = pressure && most.as_ref() == Some(&tmp);
            let sample = Sample { at, millicores, resident, most_under_pressure };
            let watched = self.watched.get_mut(&tmp).expect("watched above");
            match watched.watch.sampled(&sample, &self.limits, every) {
                Some(Turned::Hot(episode)) => {
                    turned.push(hot(watched.worker.clone(), &episode, &self.limits))
                }
                Some(Turned::Cooled { episode, .. }) => {
                    turned.push(cooled(watched.worker.clone(), &episode, false))
                }
                None => {}
            }
        }
        turned
    }

    /// The workers hot now.
    fn hot(&self) -> Vec<HotNow> {
        let hot = self.watched.values().filter_map(|watched| {
            let episode = watched.watch.hot?;
            Some(HotNow { worker: watched.worker.clone(), episode })
        });
        hot.collect()
    }
}

/// Thousandths of a core, as a person reads them: `2.5 cores`.
pub fn cores(millicores: u64) -> String {
    format!("{}.{} cores", millicores / 1000, millicores % 1000 / 100)
}

/// Why a worker turned hot, in words: what it took, and past what.
pub fn why_said(why: Why, limits: &Limits) -> String {
    match why {
        Why::Cpu => format!("past {} for {}s", cores(limits.millicores), CPU_HELD),
        Why::Memory => format!("past {} resident", measure::said(limits.resident)),
        Why::Pressure => "the host under memory pressure, holding the most".to_string(),
    }
}

/// What an episode took at the most, in words.
pub fn held_said(episode: &Episode) -> String {
    format!("{}, {} resident", cores(episode.millicores), measure::said(episode.resident))
}

fn hot(worker: String, episode: &Episode, limits: &Limits) -> What {
    What::Hot { worker, why: why_said(episode.why, limits), held: held_said(episode) }
}

fn cooled(worker: String, episode: &Episode, gone: bool) -> What {
    What::Cooled { worker, since: episode.since, held: held_said(episode), gone }
}

/// Watches every worker of the checkout `repo` is of, on a thread of its own,
/// for as long as the process lives: a sample every `SAMPLE_EVERY` seconds of
/// `second` each, timed by `clock` — the run's — and `keep` said of each turn
/// (`Sampler::sample`). `hot` is kept as the workers hot now. A second that
/// takes no time samples nothing.
pub fn watch(
    repo: &Repo,
    limits: Limits,
    second: Duration,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    keep: impl Fn(u64, What) + Send + 'static,
    hot: Hot,
) {
    if second.is_zero() {
        return;
    }
    let repo = Repo::new(repo.dir.clone());
    let every = second * SAMPLE_EVERY as u32;
    let mut sampler = Sampler { limits, listed: vec![], samples: 0, watched: BTreeMap::new() };
    std::thread::spawn(move || {
        let started = (clock(), Instant::now());
        let mut source = None;
        let mut before = Instant::now();
        loop {
            std::thread::sleep(every);
            if source.is_none() {
                source = Source::of(&repo).ok();
            }
            let Some(source) = &source else { continue };
            // The run's clock, at the pace its seconds take: a test's are short.
            let elapsed = started.1.elapsed().as_millis() as u64;
            let at = started.0 + elapsed / (second.as_millis() as u64).max(1);
            let wall_ms = before.elapsed().as_millis() as u64;
            before = Instant::now();
            for turned in sampler.sample(source, at, SAMPLE_EVERY, wall_ms) {
                keep(at, turned);
            }
            *hot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = sampler.hot();
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
        let row = "  630 94216   3024   0:01.50 /bin/zsh -c x TMPDIR=/w/.afkconvoy-tmp HOME=/u";
        let proc = parse(row).unwrap();
        assert_eq!(
            (proc.pid, proc.ppid, proc.resident, proc.cpu_ms),
            (630, 94216, 3024 * 1024, 1500)
        );
        assert_eq!(proc.line, "/bin/zsh -c x TMPDIR=/w/.afkconvoy-tmp HOME=/u");
        assert_eq!(parse("not a row"), None);
    }

    fn proc(pid: u32, ppid: u32, line: &str) -> Proc {
        Proc { pid, ppid, resident: 1024, cpu_ms: 0, line: line.to_string() }
    }

    #[test]
    fn a_workers_processes_are_those_started_with_its_tmpdir_and_all_under_them() {
        let procs = [
            proc(1, 0, "launchd"),
            proc(10, 1, "zsh -l"),
            proc(11, 10, "claude TMPDIR=/w/a/.afkconvoy-tmp HOME=/u"),
            proc(12, 11, "cargo test"),
            proc(13, 12, "rustc"),
            proc(20, 1, "claude TMPDIR=/w/a/.afkconvoy-tmp-2 HOME=/u"),
            proc(21, 1, "claude TMPDIR=/w/b/.afkconvoy-tmp"),
            proc(30, 1, "cargo TMPDIR=/w/a/.afkconvoy-tmp/"),
        ];
        let mut pids: Vec<u32> =
            tree(&procs, "/w/a/.afkconvoy-tmp").iter().map(|p| p.pid).collect();
        pids.sort();
        assert_eq!(pids, [11, 12, 13, 30]);
        assert_eq!(tree(&procs, "/w/b/.afkconvoy-tmp").len(), 1);
        assert!(tree(&procs, "/w/c/.afkconvoy-tmp").is_empty());
    }

    #[test]
    fn the_cpu_a_worker_took_is_what_its_processes_took_since_the_sample_before() {
        let mut watched = Watched { worker: "#7".into(), watch: Watch::default(), cpu: None };
        let at = |cpu: &[(u32, u64)]| -> Vec<Proc> {
            cpu.iter().map(|(pid, ms)| Proc { cpu_ms: *ms, ..proc(*pid, 1, "") }).collect()
        };
        let first = at(&[(1, 5000), (2, 700)]);
        assert_eq!(took(&mut watched, &first.iter().collect::<Vec<_>>()), (None, 2048));
        // 1 took 2000 more, 2 ended, 3 started and took 300
        let second = at(&[(1, 7000), (3, 300)]);
        assert_eq!(took(&mut watched, &second.iter().collect::<Vec<_>>()), (Some(2300), 2048));
    }

    #[test]
    fn what_a_hot_worker_took_is_said_in_cores_and_bytes() {
        assert_eq!(cores(2500), "2.5 cores");
        let limits = Limits { millicores: 4000, resident: 8 << 30 };
        assert_eq!(why_said(Why::Cpu, &limits), "past 4.0 cores for 120s");
        assert_eq!(why_said(Why::Memory, &limits), "past 8.0 GiB resident");
        let episode = Episode { since: 0, why: Why::Cpu, millicores: 6200, resident: 3 << 29 };
        assert_eq!(held_said(&episode), "6.2 cores, 1.5 GiB resident");
    }
}

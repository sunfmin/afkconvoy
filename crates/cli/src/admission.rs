//! Admission's effect half: reading the host — its cores, its load, its
//! memory free, the disk free under the checkout and how long its exclusive
//! queue takes to drain — the usage windows of the account a checkout's
//! workers start on, and the shares the host says, in its host settings. Whether admission holds is the core's to decide
//! (`afkconvoy_core::admission::holds`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use afkconvoy_core::admission::{
    Cap, Disk, Hold, LANDING_WINDOW, Landing, Own, QUEUE_BOUND, Reading, SHARES, Shares, Window,
    cap_holds, cap_limits, holds, landing_hold, queue_hold, thresholds, window_holds,
};
use afkconvoy_core::exclusive::Ticket;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::resources::{Reason, SPAN};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::host_settings;
use crate::measure;

/// The output of `program` with `args`, which must succeed.
pub(crate) fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = crate::process::output(Command::new(program).args(args))
        .with_context(|| format!("`{program}` could not be run"))?;
    if !out.status.success() {
        bail!("`{program} {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One look's host reading, with one disk observation per volume. Every
/// checkout derives its admission from these same load and memory values.
pub struct Readings {
    cores: u64,
    load: Option<u64>,
    total_load: Option<u64>,
    queue_load: Option<u64>,
    memory_free: Option<u64>,
    memory: Option<u64>,
    /// How long the exclusive queue takes to drain, in seconds; `None`: it
    /// could not be read.
    queue_drain: Option<u64>,
    /// What afkconvoy's own processes use now; `None`: the host caps none
    /// of it, or the process table could not be read.
    own: Option<Own>,
    disks: Mutex<BTreeMap<u64, Option<Disk>>>,
}

impl Readings {
    /// Reads the host-wide values once; volumes are read on first use.
    pub fn new() -> Readings {
        match crate::host_runner::shared() {
            Ok(shared) => {
                let shared = shared.unwrap_or_default();
                Self::with_queue(&shared.exclusive, &shared.exclusive_ran)
            }
            Err(_) => Self::read(|_| None, None, None),
        }
    }

    /// Reads against the runner's existing queue snapshot. Inside the runner
    /// an RPC to itself can wait for a server that has not started yet.
    pub(crate) fn with_queue(queue: &[Ticket], ran: &crate::exclusive::Ran) -> Readings {
        let settings = host_settings::of_host().unwrap_or_default();
        let drain = drain_of(queue, ran, settings.lanes(), crate::clock());
        let cap = settings.cap();
        let own = (cap != Cap::default()).then(|| own_use(queue).ok()).flatten();
        Self::read(|load| queue_load(load, queue).ok(), Some(drain), own)
    }

    fn read(
        estimate_queue: impl FnOnce(u64) -> Option<u64>,
        queue_drain: Option<u64>,
        own: Option<Own>,
    ) -> Readings {
        let total_load = load().ok();
        let queue_load = total_load.and_then(estimate_queue);
        Readings {
            cores: std::thread::available_parallelism().map_or(1, |cores| cores.get() as u64),
            load: total_load.map(|load| load.saturating_sub(queue_load.unwrap_or(0))),
            total_load,
            queue_load,
            memory_free: memory_free().ok(),
            memory: crate::hot::memory(),
            queue_drain,
            own,
            disks: Mutex::new(BTreeMap::new()),
        }
    }

    /// The host reading for a checkout at `dir`, reusing its volume's disk
    /// observation when another checkout already read it.
    pub fn at(&self, dir: &Path) -> Reading {
        use std::os::unix::fs::MetadataExt;
        let volume = std::fs::metadata(dir).map(|meta| meta.dev()).unwrap_or(0);
        let mut disks = self.disks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let disk = *disks.entry(volume).or_insert_with(|| disk(dir).ok());
        Reading { cores: self.cores, load: self.load, memory_free: self.memory_free, disk }
    }

    /// Hot-worker limits derived from this same host reading.
    pub fn hot_limits(&self, said: crate::hot::Said, dir: &Path) -> afkconvoy_core::hot::Limits {
        let reading = self.at(dir);
        crate::hot::limits_from(
            said,
            reading.cores,
            self.memory,
            reading.disk.map(|disk| disk.free),
        )
    }
}

impl Default for Readings {
    fn default() -> Self {
        Self::new()
    }
}

/// The shares this host is held to, and what was read of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub shares: Shares,
    /// Which of `shares`, and of `span`, the host said, by the host setting it
    /// said it in; the rest are the defaults.
    pub said: Vec<&'static str>,
    pub reading: Reading,
    /// Total load and its estimated queue share, in hundredths. An unknown
    /// queue share leaves the total in the admission decision.
    pub total_load: Option<u64>,
    pub queue_load: Option<u64>,
    /// How long, in seconds, admission holds back for one reason before the
    /// host calls for a watch: the host's, or `SPAN`.
    pub span: u64,
    /// How long the host's exclusive queue takes to drain, in seconds, and
    /// past how long it holds: `QUEUE_BOUND`. `None`: it could not be read.
    pub queue_drain: Option<u64>,
    pub queue_bound: u64,
    /// The account the checkout's workers start on, as its windows say;
    /// `None`: none was read — its harness reports none, or nothing asked.
    pub usage: Option<Usage>,
    /// The share of the host afkconvoy may use itself, what its own
    /// processes use now — `None`: not read — and the host's memory, in
    /// bytes, where it was read.
    pub cap: Cap,
    pub own: Option<Own>,
    pub memory: Option<u64>,
    /// What the checkout's candidate chain answered in the last
    /// `LANDING_WINDOW`, as this host's tick log of it says; `None`: it
    /// could not be read (docs/adr/0076).
    pub landing: Option<Landing>,
}

/// What admission reads of the account a checkout's next worker starts on:
/// the windows its workers' newest records say, and how many workers run on
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub account: Harness,
    pub windows: Vec<Window>,
    pub running: usize,
}

impl Admission {
    /// The shares the host settings say and what is read of the host now,
    /// for a checkout at `dir`. Settings that cannot be read are an error,
    /// not the defaults.
    pub fn read(dir: &Path) -> Result<Admission> {
        Self::read_from(dir, &Readings::new())
    }

    /// The shares the host settings say, with the checkout's reading from one
    /// shared look.
    pub fn read_from(dir: &Path, readings: &Readings) -> Result<Admission> {
        let mut admission = Admission::of(&host_settings::of_host()?, readings.at(dir));
        admission.total_load = readings.total_load;
        admission.queue_load = readings.queue_load;
        admission.queue_drain = readings.queue_drain;
        admission.own = readings.own;
        admission.memory = readings.memory;
        Ok(admission)
    }

    /// The shares `settings` say, with `reading`.
    fn of(settings: &host_settings::Settings, reading: Reading) -> Admission {
        let mut shares = SHARES;
        let mut span = SPAN;
        let mut said = vec![];
        for (name, setting, share) in [
            ("admit_load", settings.admit_load, &mut shares.load),
            ("admit_memory", settings.admit_memory, &mut shares.memory),
            ("admit_disk", settings.admit_disk, &mut shares.disk),
            ("admit_inspect_after", settings.admit_inspect_after, &mut span),
        ] {
            if let Some(setting) = setting {
                *share = setting;
                said.push(name);
            }
        }
        Admission {
            shares,
            said,
            total_load: reading.load,
            queue_load: Some(0),
            reading,
            span,
            queue_drain: None,
            queue_bound: QUEUE_BOUND,
            usage: None,
            cap: settings.cap(),
            own: None,
            memory: None,
            landing: None,
        }
    }

    /// This admission, judging the candidate chain of the checkout at `dir`
    /// too, as its tick log has it up to `now` (docs/adr/0076).
    pub fn with_landing(self, dir: &Path, now: u64) -> Admission {
        Admission { landing: landing_of(dir, now), ..self }
    }

    /// This admission, judging the account `usage` too.
    pub fn with_usage(self, usage: Option<Usage>) -> Admission {
        Admission { usage, ..self }
    }

    /// The thresholds the host is past, then the account's windows that
    /// would run out before they reset, then the queue, then the share of
    /// the host afkconvoy may use itself, then the checkout's candidate chain:
    /// none, and new workers start.
    pub fn holds(&self) -> Vec<Hold> {
        let mut held = holds(self.shares, &self.reading);
        if let Some(usage) = &self.usage {
            held.extend(window_holds(&usage.windows, |_| usage.running));
        }
        held.extend(queue_hold(self.queue_drain, self.queue_bound));
        held.extend(cap_holds(self.cap, self.own, self.reading.cores, self.memory));
        held.extend(landing_hold(self.landing, LANDING_WINDOW));
        held
    }

    /// The reasons admission holds back for now that call for the host's
    /// watch once held past its span: none, and none calls. The candidate
    /// chain is none of them: it drains as the workers already running land,
    /// and nothing of the host is the watch's to mend for it
    /// (docs/adr/0076).
    pub fn reasons(&self) -> Vec<Reason> {
        let reasons = self.holds().iter().map(Reason::of).collect::<Vec<_>>();
        reasons.into_iter().filter(|reason| *reason != Reason::Landing).collect()
    }

    /// `hold` as a tick's line and `plan` say it.
    pub fn said(&self, hold: &Hold) -> String {
        match hold {
            Hold::Load { load, limit } => format!(
                "the load {} is past {}, {}% of its {} cores",
                hundredths(*load),
                hundredths(*limit),
                self.shares.load,
                self.reading.cores
            ),
            Hold::Memory { free, floor } => {
                format!("{free}% of its memory is free, under {floor}%")
            }
            Hold::Disk { free, floor } => format!(
                "{} of the checkout's disk is free, under {}, {}% of it",
                measure::said(*free),
                measure::said(*floor),
                self.shares.disk
            ),
            Hold::Window { account, window, used, projected, running, resets_in } => format!(
                "the `{window}` window of the {account} account is {used}% used and resets in \
                 {}: at its rate, with {running} worker(s) on it and one more, it would be \
                 {projected}% used by then",
                minutes(*resets_in)
            ),
            Hold::Queue { drain, bound } => format!(
                "the exclusive queue would take {} to drain, by its commands' recent run times, \
                 past {}",
                minutes(*drain),
                minutes(*bound)
            ),
            Hold::OwnCores { used, limit } => format!(
                "afkconvoy's own processes take {} cores, at or past {}, the {}% of its {} cores \
                 it may use",
                hundredths(*used),
                hundredths(*limit),
                self.cap.cores.unwrap_or(100),
                self.reading.cores
            ),
            Hold::OwnMemory { resident, limit } => format!(
                "afkconvoy's own processes hold {} resident, at or past {}, the {}% of its memory \
                 it may use",
                measure::said(*resident),
                measure::said(*limit),
                self.cap.memory.unwrap_or(100)
            ),
            Hold::Landing { conflicts, landed, window } => format!(
                "landing contention, {conflicts} conflicts in the last {} against {landed} \
                 landing(s): its candidate chain cannot keep up, and starts no new worker of \
                 this repo until it drains",
                window_said(*window)
            ),
        }
    }

    /// What was read of the host, in words, as the panel shows it: what
    /// could not be read is left out.
    pub fn read_said(&self) -> Vec<String> {
        let reading = &self.reading;
        let mut said = vec![];
        if let Some(load) = reading.load {
            said.push(format!("a load of {} on {} cores", hundredths(load), reading.cores));
        }
        if let Some(free) = reading.memory_free {
            said.push(format!("{free}% of its memory free"));
        }
        if let Some(disk) = reading.disk {
            said.push(format!("{} of the checkout's disk free", measure::said(disk.free)));
        }
        if let Some(drain) = self.queue_drain {
            said.push(format!("an exclusive queue {} from drained", minutes(drain)));
        }
        said
    }

    /// The thresholds in force, the host's reading and what holds, as `plan`
    /// and `doctor` answer them.
    pub fn json(&self) -> Value {
        let limits = thresholds(self.shares, &self.reading);
        let reading = &self.reading;
        let holds: Vec<String> = self.holds().iter().map(|hold| self.said(hold)).collect();
        json!({
            "admits": holds.is_empty(),
            "held_back_by": holds,
            "shares": {
                "load_percent_of_cores": self.shares.load,
                "memory_free_percent": self.shares.memory,
                "disk_free_percent": self.shares.disk,
                "inspect_after_seconds": self.span,
                "said_by_host": self.said,
            },
            "thresholds": {
                "load": limits.load as f64 / 100.0,
                "memory_free_percent": limits.memory_free,
                "disk_free_bytes": limits.disk_free,
            },
            "reading": {
                "cores": reading.cores,
                "load": reading.load.map(|load| load as f64 / 100.0),
                "total_load": self.total_load.map(|load| load as f64 / 100.0),
                "queue_load": self.queue_load.map(|load| load as f64 / 100.0),
                "other_load": reading.load.map(|load| load as f64 / 100.0),
                "memory_free_percent": reading.memory_free,
                "disk_free_bytes": reading.disk.map(|disk| disk.free),
                "disk_size_bytes": reading.disk.map(|disk| disk.size),
            },
            "queue": {"drain_seconds": self.queue_drain, "bound_seconds": self.queue_bound},
            "share": self.share_json(),
            "landing": self.landing.map(|landing| json!({
                "conflicts": landing.conflicts,
                "landed": landing.landed,
                "window_seconds": LANDING_WINDOW,
            })),
            "usage": self.usage.as_ref().map(|usage| json!({
                "account": usage.account.key(),
                "running": usage.running,
                "windows": usage.windows,
            })),
        })
    }
}

impl Admission {
    /// The share of the host afkconvoy may use itself, what it comes to and
    /// what its own processes use now: {max_share_of_cores,
    /// max_share_of_memory, cores_limit, memory_limit_bytes, own_cores,
    /// own_resident_bytes}, each null where the host caps none or it was not
    /// read.
    fn share_json(&self) -> Value {
        let (cpu, resident) = cap_limits(self.cap, self.reading.cores, self.memory);
        json!({
            "max_share_of_cores": self.cap.cores,
            "max_share_of_memory": self.cap.memory,
            "cores_limit": cpu.map(|cpu| cpu as f64 / 100.0),
            "memory_limit_bytes": resident,
            "own_cores": self.own.map(|own| own.cpu as f64 / 100.0),
            "own_resident_bytes": self.own.map(|own| own.resident),
        })
    }
}

/// What the candidate chain of the checkout at `dir` answered in the
/// `LANDING_WINDOW` up to `now`, as this host's tick log of its repo has the
/// `land` turns' outcomes; `None`: there is no log to read.
pub(crate) fn landing_of(dir: &Path, now: u64) -> Option<Landing> {
    let log = crate::log::Log::of(&crate::git::Repo::new(dir.to_path_buf())).ok()?;
    Some(landing_in(&log.read()?, now))
}

/// What the `land` lines of `entries` say of the `LANDING_WINDOW` up to
/// `now`: each turn's end, its outcome word, and never a step it said on
/// its way, nor one that failed.
pub(crate) fn landing_in(entries: &[crate::log::Entry], now: u64) -> Landing {
    let outcomes = entries.iter().filter_map(|entry| match &entry.what {
        crate::log::What::Command { command, outcome, failed: false, .. } if command == "land" => {
            Some((entry.at, outcome.as_str()))
        }
        _ => None,
    });
    Landing::tally(outcomes, now, LANDING_WINDOW)
}

/// A window of `seconds`, in words: `hour` for an hour.
fn window_said(seconds: u64) -> String {
    match seconds {
        3600 => "hour".to_string(),
        _ => minutes(seconds),
    }
}

/// What afkconvoy's own processes on this host use now, in one reading of
/// the process table: its host runner and keeper and all under them, the
/// processes its fleets started (`hot::fleets`), and the running work of
/// its exclusive queue, `queue` — a cut's build among it.
fn own_use(queue: &[Ticket]) -> Result<Own> {
    let procs = crate::hot::listed_checked()?;
    let ours = |worktree: &Path| {
        crate::orca::Worktree::at(worktree.to_path_buf(), String::new()).holds_a_worker()
    };
    let mut pids = crate::hot::fleets(&procs, ours);
    pids.extend(crate::exclusive::running_work(queue, &procs).pids);
    let runner = [crate::host_runner::pid(), crate::host_runner::keeper()];
    pids.extend(under(&procs, runner.into_iter().flatten()));
    Ok(own_of(&procs, &pids))
}

/// The pids of `roots` and of every process of `procs` under one of them.
fn under(procs: &[crate::hot::Proc], roots: impl IntoIterator<Item = u32>) -> BTreeSet<u32> {
    let mut found: BTreeSet<u32> = roots.into_iter().collect();
    loop {
        let more: Vec<u32> = procs
            .iter()
            .filter(|proc| found.contains(&proc.ppid) && !found.contains(&proc.pid))
            .map(|proc| proc.pid)
            .collect();
        if more.is_empty() {
            return found;
        }
        found.extend(more);
    }
}

/// What the processes `of` of `procs` use: the share of a core each takes
/// now — `ps` says it in hundredths of a percent, a ten-thousandth of a core
/// — in hundredths of a core, and what each holds resident.
fn own_of(procs: &[crate::hot::Proc], of: &std::collections::HashSet<u32>) -> Own {
    let ours = procs.iter().filter(|proc| of.contains(&proc.pid));
    let (share, resident) = ours.fold((0u64, 0u64), |(share, resident), proc| {
        (share.saturating_add(proc.cpu_share), resident.saturating_add(proc.resident))
    });
    Own { cpu: share / 100, resident }
}

/// Attribute the one-minute load in proportion to CPU currently used by the
/// queue's running work, as the exclusive queue finds it in one reading of
/// the process table. This is an estimate: the kernel exposes no per-tree
/// load average. On an unreadable queue or process table, subtract nothing.
fn queue_load(load: u64, queue: &[Ticket]) -> Result<u64> {
    if !queue.iter().any(Ticket::running) {
        return Ok(0);
    }
    let procs = crate::hot::listed_checked()?;
    let work = crate::exclusive::running_work(queue, &procs);
    anyhow::ensure!(work.whole, "queue process table incomplete");
    Ok(share_of(load, &procs, &work.pids))
}

/// The part of `load` that the processes `of` take, by the share of a core
/// each of `procs` takes now.
fn share_of(load: u64, procs: &[crate::hot::Proc], of: &BTreeSet<u32>) -> u64 {
    let total: u64 = procs.iter().map(|proc| proc.cpu_share).sum();
    let queued: u64 =
        procs.iter().filter(|proc| of.contains(&proc.pid)).map(|proc| proc.cpu_share).sum();
    if total == 0 {
        return 0;
    }
    ((load as f64 * queued as f64 / total as f64).round() as u64).min(load)
}

/// How long the queue `queue` takes to drain, in seconds, by the recent run
/// times `ran` keeps of its commands, in the lanes `lanes` give it
/// (`admission::drain`): what is left of each running command, the whole of
/// each waiting one. A command no run of which is kept counts for nothing.
fn drain_of(
    queue: &[Ticket],
    ran: &crate::exclusive::Ran,
    lanes: afkconvoy_core::exclusive::Lanes,
    now: u64,
) -> u64 {
    let commands: Vec<(String, u64, u64)> = queue
        .iter()
        .filter_map(|ticket| {
            let takes = ran.takes(ticket)?;
            let left = match ticket.ran {
                Some(began) => takes.saturating_sub(now.saturating_sub(began)),
                None => takes,
            };
            let lane = afkconvoy_core::exclusive::lane(ticket);
            let places = match lane.as_str() {
                "worker" => lanes.worker,
                "fast" => lanes.fast,
                "slow" => lanes.slow,
                _ => 1,
            };
            Some((lane, places, left))
        })
        .collect();
    afkconvoy_core::admission::drain(&commands)
}

/// The windows `said` of the account of `account`, as admission reads them
/// `now`: each with its length and how long until it resets. A window that
/// says no reset, or has reset since it was said, is left out; so is one
/// whose length neither its engine says nor its name tells.
pub fn windows_of(
    account: Harness,
    said: &[afkconvoy_core::harness::window::Window],
    now: u64,
) -> Vec<Window> {
    said.iter()
        .filter_map(|window| {
            let resets_in = window.resets_at?.checked_sub(now).filter(|left| *left > 0)?;
            let minutes = window.minutes.or(match window.name.as_str() {
                "five_hour" | "limit_5h" => Some(5 * 60),
                "seven_day" | "seven_day_opus" | "seven_day_sonnet" => Some(7 * 24 * 60),
                _ => None,
            })?;
            Some(Window {
                account: account.key().to_string(),
                window: window.name.clone(),
                used: window.used_percent.max(0.0).round() as u64,
                length: minutes.saturating_mul(60),
                resets_in,
            })
        })
        .collect()
}

/// `seconds` in whole minutes, as a span is said: `14 min`.
fn minutes(seconds: u64) -> String {
    format!("{} min", seconds.div_ceil(60))
}

/// `hundredths` as a load is written: `105.43`.
fn hundredths(hundredths: u64) -> String {
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// The 1-minute load average, in hundredths: what `sysctl vm.loadavg` says
/// first, `{ 2.34 2.10 1.98 }`, or `/proc/loadavg` where there is one.
fn load() -> Result<u64> {
    let said = match std::fs::read_to_string("/proc/loadavg") {
        Ok(said) => said,
        Err(_) => output("sysctl", &["-n", "vm.loadavg"])?,
    };
    first_load(&said).with_context(|| format!("the load average was said as {said:?}"))
}

fn first_load(said: &str) -> Option<u64> {
    let first = said.split_whitespace().find(|word| *word != "{")?;
    let load: f64 = first.parse().ok()?;
    (load >= 0.0).then(|| (load * 100.0).round() as u64)
}

/// The share of memory free, in percent: the kernel's own count of what is
/// available — `sysctl kern.memorystatus_level`, or `MemAvailable` of
/// `MemTotal` in `/proc/meminfo` where there is one.
fn memory_free() -> Result<u64> {
    if let Ok(said) = std::fs::read_to_string("/proc/meminfo") {
        return meminfo(&said).context("/proc/meminfo says no MemAvailable and MemTotal");
    }
    let said = output("sysctl", &["-n", "kern.memorystatus_level"])?;
    said.trim().parse().with_context(|| format!("`sysctl kern.memorystatus_level` said {said:?}"))
}

fn meminfo(said: &str) -> Option<u64> {
    let field = |name: &str| -> Option<u64> {
        let line = said.lines().find(|line| line.starts_with(name))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    let (available, total) = (field("MemAvailable:")?, field("MemTotal:")?);
    (total > 0).then(|| available.saturating_mul(100) / total)
}

/// The disk of the volume `dir` is on, as `df -Pk` says it.
fn disk(dir: &Path) -> Result<Disk> {
    let said = output("df", &["-Pk", &dir.display().to_string()])?;
    df(&said).with_context(|| format!("`df -Pk` said {said:?}"))
}

fn df(said: &str) -> Option<Disk> {
    // Filesystem 1024-blocks Used Available Capacity Mounted-on
    let fields: Vec<&str> = said.lines().nth(1)?.split_whitespace().collect();
    let kib = |at: usize| -> Option<u64> { fields.get(at)?.parse::<u64>().ok() };
    Some(Disk { free: kib(3)?.saturating_mul(1024), size: kib(1)?.saturating_mul(1024) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_settings::To;

    #[test]
    fn the_host_is_read_as_sysctl_proc_and_df_say_it() {
        assert_eq!(first_load("{ 105.43 84.89 85.03 }\n"), Some(10543));
        assert_eq!(first_load("0.52 0.58 0.59 1/467 12345\n"), Some(52));
        assert_eq!(first_load("nothing"), None);
        let meminfo_said =
            "MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    4000000 kB\n";
        assert_eq!(meminfo(meminfo_said), Some(25));
        assert_eq!(meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
        let df_said = "Filesystem   1024-blocks      Used Available Capacity  Mounted on\n\
                       /dev/disk3s5   971350180 894451764  45889612    96%    /System/Volumes/Data\n";
        assert_eq!(df(df_said), Some(Disk { free: 45_889_612 * 1024, size: 971_350_180 * 1024 }));
    }

    #[test]
    fn each_threshold_is_said_with_what_was_read() {
        let admission = Admission {
            shares: SHARES,
            said: vec![],
            reading: Reading { cores: 14, load: Some(10543), memory_free: Some(4), disk: None },
            total_load: Some(10543),
            queue_load: Some(0),
            span: SPAN,
            queue_drain: None,
            queue_bound: afkconvoy_core::admission::QUEUE_BOUND,
            usage: None,
            cap: Cap::default(),
            own: None,
            memory: None,
            landing: None,
        };
        let said: Vec<String> = admission.holds().iter().map(|hold| admission.said(hold)).collect();
        assert_eq!(
            said,
            [
                "the load 105.43 is past 28.00, 200% of its 14 cores",
                "4% of its memory is free, under 10%"
            ]
        );
        assert_eq!(admission.json()["admits"], false);
        assert_eq!(admission.reasons(), [Reason::Load, Reason::Memory]);
        assert_eq!(admission.json()["shares"]["inspect_after_seconds"], 600);
    }

    #[test]
    fn the_shares_are_those_the_host_settings_say_and_the_defaults_else() {
        let reading = Reading { cores: 14, load: Some(5000), memory_free: Some(4), disk: None };
        let unsaid = Admission::of(&host_settings::Settings::default(), reading);
        assert_eq!((unsaid.shares, unsaid.span), (SHARES, SPAN));
        assert!(unsaid.said.is_empty());
        let settings = host_settings::Settings::default().with(&host_settings::Named {
            admit_load: Some(To::Value(400)),
            admit_memory: Some(To::Value(0)),
            admit_inspect_after: Some(To::Value(900)),
            ..host_settings::Named::default()
        });
        let said = Admission::of(&settings, reading);
        assert_eq!(said.shares, Shares { load: 400, memory: 0, disk: SHARES.disk });
        assert_eq!(said.span, 900);
        assert_eq!(said.said, ["admit_load", "admit_memory", "admit_inspect_after"]);
        assert!(said.holds().is_empty(), "400% of 14 cores is 56, and 0% free never holds");
    }
}

#[cfg(test)]
mod readings_tests {
    use super::*;
    use afkconvoy_core::harness::window::Window as Said;

    fn at_ease() -> Admission {
        let reading = Reading { cores: 14, load: Some(100), memory_free: Some(60), disk: None };
        Admission::of(&host_settings::Settings::default(), reading)
    }

    fn ticket(number: u64, command: &str, ran: Option<u64>) -> Ticket {
        serde_json::from_value(json!({
            "number": number, "pid": 1, "command": command, "dir": "/w",
            "asked": 0, "ran": ran,
        }))
        .unwrap()
    }

    #[test]
    fn a_window_that_would_run_out_holds_and_is_said_with_its_numbers() {
        let now = 100_000;
        let said = [
            Said {
                name: "primary".into(),
                used_percent: 60.0,
                minutes: Some(300),
                resets_at: Some(now + 9000),
            },
            // reset since it was said: left out
            Said {
                name: "secondary".into(),
                used_percent: 99.0,
                minutes: Some(10_080),
                resets_at: Some(now - 1),
            },
            // no length its engine says nor its name tells: left out
            Said {
                name: "other".into(),
                used_percent: 99.0,
                minutes: None,
                resets_at: Some(now + 60),
            },
        ];
        let windows = windows_of(Harness::Codex, &said, now);
        assert_eq!(windows.len(), 1);
        assert_eq!((windows[0].used, windows[0].length, windows[0].resets_in), (60, 18_000, 9000));
        let usage = Usage { account: Harness::Codex, windows, running: 2 };
        let held = at_ease().with_usage(Some(usage));
        let holds = held.holds();
        assert_eq!(holds.len(), 1);
        assert_eq!(
            held.said(&holds[0]),
            "the `primary` window of the codex account is 60% used and resets in 150 min: at its \
             rate, with 2 worker(s) on it and one more, it would be 150% used by then"
        );
        assert_eq!(held.reasons(), [Reason::Window]);
        assert_eq!(held.json()["usage"]["running"], 2);
        assert!(at_ease().with_usage(None).holds().is_empty(), "no window: the host alone");
    }

    /// A process of `pid` under `ppid`, taking `percent` of a core and
    /// holding `gib` GiB resident.
    fn proc(pid: u32, ppid: u32, percent: u64, gib: u64) -> crate::hot::Proc {
        crate::hot::Proc {
            pid,
            ppid,
            resident: gib << 30,
            cpu_ms: 0,
            cpu_share: percent * 100,
            written: 0,
            line: String::new(),
            cwd: None,
        }
    }

    #[test]
    fn with_the_share_set_admission_refuses_a_start_that_would_take_afkconvoy_over_it() {
        // A 14-core, 48-GiB Hub: the host runner at 65% of a core, a gate's
        // build under a worker at 600%, and a co-resident system's own work.
        let procs = [
            proc(100, 1, 65, 1),
            proc(101, 100, 10, 1),
            proc(200, 1, 50, 2),
            proc(201, 200, 600, 6),
            proc(300, 1, 400, 20),
        ];
        let runner = under(&procs, [100]);
        assert_eq!(runner, BTreeSet::from([100, 101]), "the runner and all under it");
        let ours: std::collections::HashSet<u32> =
            runner.into_iter().chain(under(&procs, [200])).collect();
        let own = own_of(&procs, &ours);
        assert_eq!(own, Own { cpu: 725, resident: 10 << 30 }, "not the other system's");

        let settings = |cores: Option<u64>, memory: Option<u64>| {
            host_settings::Settings::default().with(&host_settings::Named {
                max_share_of_cores: cores.map(host_settings::To::Value),
                max_share_of_memory: memory.map(host_settings::To::Value),
                ..host_settings::Named::default()
            })
        };
        let at = |settings: &host_settings::Settings| Admission {
            own: Some(own),
            memory: Some(48 << 30),
            ..Admission::of(settings, at_ease().reading)
        };
        // Half the cores is 7: afkconvoy's 7.25 leaves no room for a start,
        // where the host's load alone (1.00 of 14) would admit one.
        let half = at(&settings(Some(50), None));
        let holds = half.holds();
        assert_eq!(holds, [Hold::OwnCores { used: 725, limit: 700 }]);
        assert_eq!(
            half.said(&holds[0]),
            "afkconvoy's own processes take 7.25 cores, at or past 7.00, the 50% of its 14 \
             cores it may use"
        );
        assert_eq!(half.reasons(), [Reason::Share]);
        assert_eq!(half.json()["admits"], false);
        assert_eq!(half.json()["share"]["max_share_of_cores"], 50);
        assert_eq!(half.json()["share"]["own_cores"], 7.25);
        // More room, and it admits; no cap, and its own use is never asked.
        assert!(at(&settings(Some(60), None)).holds().is_empty(), "8.40 cores of room");
        assert!(at(&settings(None, None)).holds().is_empty());
        // A fifth of 48 GiB is under its 10 GiB resident.
        let memory = at(&settings(None, Some(20)));
        assert_eq!(memory.reasons(), [Reason::Share]);
        assert!(memory.said(&memory.holds()[0]).contains("the 20% of its memory it may use"));
    }

    #[test]
    fn a_queue_that_would_take_past_its_bound_to_drain_holds() {
        let mut ran = crate::exclusive::Ran::default();
        let gate = ticket(1, "./gate.sh", None);
        for seconds in [500, 600, 700] {
            ran.ran(crate::exclusive::Ran::key(&gate), seconds);
        }
        assert_eq!(ran.takes(&gate), Some(600));
        let lanes = afkconvoy_core::exclusive::Lanes { worker: 1, fast: 1, slow: 1 };
        // one running 100s of its 600, two waiting: 500 + 600 + 600
        let queue = [
            ticket(1, "./gate.sh", Some(900)),
            ticket(2, "./gate.sh", None),
            ticket(3, "./gate.sh", None),
        ];
        assert_eq!(drain_of(&queue, &ran, lanes, 1000), 1700);
        assert_eq!(drain_of(&[ticket(4, "make", None)], &ran, lanes, 1000), 0, "never ran");
        let held = Admission { queue_drain: Some(1700), ..at_ease() };
        let holds = held.holds();
        assert_eq!(
            held.said(&holds[0]),
            "the exclusive queue would take 29 min to drain, by its commands' recent run times, \
             past 15 min"
        );
        assert_eq!(held.json()["queue"]["drain_seconds"], 1700);
        assert!(Admission { queue_drain: Some(QUEUE_BOUND), ..at_ease() }.holds().is_empty());
    }

    #[test]
    fn the_queue_keeps_the_last_runs_of_each_command() {
        let mut ran = crate::exclusive::Ran::default();
        for seconds in 1..=7 {
            ran.ran("cmd".into(), seconds);
        }
        assert_eq!(ran.0["cmd"], [3, 4, 5, 6, 7]);
    }
}

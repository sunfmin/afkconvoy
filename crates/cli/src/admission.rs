//! Admission's effect half: reading the host — its cores, its load, its
//! memory free and the disk free under the checkout — and the shares the host
//! says, in its host settings. Whether admission holds is the core's to decide
//! (`afkconvoy_core::admission::holds`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use afkconvoy_core::admission::{Disk, Hold, Reading, SHARES, Shares, holds, thresholds};
use afkconvoy_core::exclusive::Ticket;
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
    disks: Mutex<BTreeMap<u64, Option<Disk>>>,
}

impl Readings {
    /// Reads the host-wide values once; volumes are read on first use.
    pub fn new() -> Readings {
        Self::read(|load| {
            crate::exclusive::standing().ok().and_then(|queue| queue_load(load, &queue).ok())
        })
    }

    /// Reads against the runner's existing queue snapshot. Inside the runner
    /// an RPC to itself can wait for a server that has not started yet.
    pub(crate) fn with_queue(queue: &[Ticket]) -> Readings {
        Self::read(|load| queue_load(load, queue).ok())
    }

    fn read(estimate_queue: impl FnOnce(u64) -> Option<u64>) -> Readings {
        let total_load = load().ok();
        let queue_load = total_load.and_then(estimate_queue);
        Readings {
            cores: std::thread::available_parallelism().map_or(1, |cores| cores.get() as u64),
            load: total_load.map(|load| load.saturating_sub(queue_load.unwrap_or(0))),
            total_load,
            queue_load,
            memory_free: memory_free().ok(),
            memory: crate::hot::memory(),
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
        Admission { shares, said, total_load: reading.load, queue_load: Some(0), reading, span }
    }

    /// The thresholds the host is past: none, and new workers start.
    pub fn holds(&self) -> Vec<Hold> {
        holds(self.shares, &self.reading)
    }

    /// The reasons admission holds back for now: none, and it admits.
    pub fn reasons(&self) -> Vec<Reason> {
        self.holds().iter().map(Reason::of).collect()
    }

    /// `hold` as a tick's line and `plan` say it.
    pub fn said(&self, hold: &Hold) -> String {
        match *hold {
            Hold::Load { load, limit } => format!(
                "the load {} is past {}, {}% of its {} cores",
                hundredths(load),
                hundredths(limit),
                self.shares.load,
                self.reading.cores
            ),
            Hold::Memory { free, floor } => {
                format!("{free}% of its memory is free, under {floor}%")
            }
            Hold::Disk { free, floor } => format!(
                "{} of the checkout's disk is free, under {}, {}% of it",
                measure::said(free),
                measure::said(floor),
                self.shares.disk
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
        })
    }
}

/// Attribute the one-minute load in proportion to CPU currently used by the
/// running queue trees. This is an estimate: the kernel exposes no per-tree
/// load average. On an unreadable queue or process table, subtract nothing.
fn queue_load(load: u64, queue: &[Ticket]) -> Result<u64> {
    let roots: BTreeSet<u32> =
        queue.iter().filter(|ticket| ticket.ran.is_some()).map(|ticket| ticket.pid).collect();
    if roots.is_empty() {
        return Ok(0);
    }
    let said = output("ps", &["-A", "-o", "pid=,ppid=,pcpu="])?;
    let rows: Vec<(u32, u32, f64)> = said
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (pid, parent, cpu) = (
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse::<f64>().ok()?,
            );
            (cpu.is_finite() && cpu >= 0.0).then_some((pid, parent, cpu))
        })
        .collect();
    anyhow::ensure!(
        !rows.is_empty() && roots.iter().all(|root| rows.iter().any(|row| row.0 == *root)),
        "queue process table incomplete"
    );
    let mut tree = roots;
    loop {
        let before = tree.len();
        for &(pid, parent, _) in &rows {
            if tree.contains(&parent) {
                tree.insert(pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    let total: f64 = rows.iter().map(|row| row.2).sum();
    let queued: f64 = rows.iter().filter(|row| tree.contains(&row.0)).map(|row| row.2).sum();
    if total <= 0.0 {
        return Ok(0);
    }
    Ok(((load as f64 * queued / total).round() as u64).min(load))
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

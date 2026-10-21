//! Admission's effect half: reading the host — its cores, its load, its
//! memory free and the disk free under the checkout — and the shares the host
//! says, from its environment. Whether admission holds is the core's to decide
//! (`afkconvoy_core::admission::holds`).

use std::path::Path;

use afkconvoy_core::admission::{Disk, Hold, Reading, SHARES, Shares, holds, thresholds};
use afkconvoy_core::host_inspection::{Reason, SPAN};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::host_sleep::output;
use crate::measure;

/// The environment variables a host says its own shares in, each a whole
/// percent: of its cores the load may reach, of its memory and of the
/// checkout's disk that must stay free.
pub const LOAD_ENV: &str = "AFKCONVOY_ADMIT_LOAD";
pub const MEMORY_ENV: &str = "AFKCONVOY_ADMIT_MEMORY";
pub const DISK_ENV: &str = "AFKCONVOY_ADMIT_DISK";
/// The environment variable a host says in, in whole seconds, how long
/// admission holds back for one reason before the host is inspected.
pub const INSPECT_ENV: &str = "AFKCONVOY_ADMIT_INSPECT_AFTER";

/// The shares this host is held to, and what was read of it.
pub struct Admission {
    pub shares: Shares,
    /// Which of `shares` the host said, by the environment variable it said
    /// it in; the rest are the defaults.
    pub said: Vec<&'static str>,
    pub reading: Reading,
    /// How long, in seconds, admission holds back for one reason before the
    /// host is inspected: the host's, or `SPAN`.
    pub span: u64,
}

impl Admission {
    /// The shares the host says and what is read of it now, for a checkout at
    /// `dir`. A share said wrong is an error, not a default.
    pub fn read(dir: &Path) -> Result<Admission> {
        let mut shares = SHARES;
        let mut said = vec![];
        for (name, share) in [
            (LOAD_ENV, &mut shares.load),
            (MEMORY_ENV, &mut shares.memory),
            (DISK_ENV, &mut shares.disk),
        ] {
            if let Some(value) = std::env::var_os(name) {
                *share = percent(name, &value.to_string_lossy())?;
                said.push(name);
            }
        }
        let mut span = SPAN;
        if let Some(value) = std::env::var_os(INSPECT_ENV) {
            span = seconds(INSPECT_ENV, &value.to_string_lossy())?;
            said.push(INSPECT_ENV);
        }
        Ok(Admission { shares, said, reading: read_host(dir), span })
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
                "memory_free_percent": reading.memory_free,
                "disk_free_bytes": reading.disk.map(|disk| disk.free),
                "disk_size_bytes": reading.disk.map(|disk| disk.size),
            },
        })
    }
}

/// A share as the host says it: a whole percent.
fn percent(name: &str, value: &str) -> Result<u64> {
    match value.trim().parse::<u64>() {
        Ok(share) => Ok(share),
        Err(_) => bail!("{name}={value:?} is no share: say a whole percent, as {name}=150"),
    }
}

/// A span as the host says it: whole seconds.
fn seconds(name: &str, value: &str) -> Result<u64> {
    match value.trim().parse::<u64>() {
        Ok(seconds) => Ok(seconds),
        Err(_) => bail!("{name}={value:?} is no span: say whole seconds, as {name}=900"),
    }
}

/// `hundredths` as a load is written: `105.43`.
fn hundredths(hundredths: u64) -> String {
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// What is read of the host now, the disk of the checkout at `dir`. What
/// cannot be read is `None`, and holds nothing back.
fn read_host(dir: &Path) -> Reading {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get() as u64);
    Reading { cores, load: load().ok(), memory_free: memory_free().ok(), disk: disk(dir).ok() }
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
    fn a_share_said_wrong_is_an_error() {
        assert_eq!(percent(LOAD_ENV, " 150\n").unwrap(), 150);
        let error = percent(DISK_ENV, "5%").unwrap_err().to_string();
        assert!(error.contains("AFKCONVOY_ADMIT_DISK=\"5%\" is no share"), "{error}");
        assert_eq!(seconds(INSPECT_ENV, "900").unwrap(), 900);
        assert!(seconds(INSPECT_ENV, "10m").is_err());
    }
}

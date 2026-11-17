//! How busy a host is, as the holder of its exclusive queue starts a command
//! by it (docs/adr/0088, docs/adr/0058): its cores by the kernel's CPU ticks
//! over the last minute, or the load average over its cores where those
//! cannot be read; and its memory free, where it is under a floor its caller
//! says. afkconvoy's host runner reads its host through it, and so may
//! another program that embeds `afkconvoy_gate::holder::Holder`: there is
//! one reader of the kernel's ticks. Its API names no fleet concept.

pub mod cpu;
pub mod spawn;

use std::process::{Command, Stdio};

use afkconvoy_gate::exclusive::{By, Reading, Short};

pub use cpu::Minute;

/// How busy this host is at `now`, in epoch seconds: its cores by the
/// kernel's ticks over the last minute (`minute`), or the load average alone
/// over its cores where those cannot be read; and its memory free, where it
/// is under `floor`, in percent of the host's. `None`: neither can be read.
pub fn reading(minute: &mut Minute, now: u64, floor: u64) -> Option<Reading> {
    let (busy, by, since) = match minute.read(now) {
        Some((busy, since)) => (busy, By::Ticks, since),
        None => {
            let busy = (load()? / cores().max(1)).min(100);
            (busy, By::Load, now.saturating_sub(cpu::WINDOW.as_secs()))
        }
    };
    let memory = memory_free().filter(|free| *free < floor).map(|free| Short { free, floor });
    Some(Reading { busy, by, since, taken: now, memory })
}

/// This host's logical cores.
pub fn cores() -> u64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u64)
}

/// The 1-minute load average, in hundredths: what `sysctl vm.loadavg` says
/// first, `{ 2.34 2.10 1.98 }`, or `/proc/loadavg` where there is one.
/// `None`: it cannot be read.
pub fn load() -> Option<u64> {
    let said = match std::fs::read_to_string("/proc/loadavg") {
        Ok(said) => said,
        Err(_) => sysctl("vm.loadavg")?,
    };
    first_load(&said)
}

fn first_load(said: &str) -> Option<u64> {
    let first = said.split_whitespace().find(|word| *word != "{")?;
    let load: f64 = first.parse().ok()?;
    (load >= 0.0).then(|| (load * 100.0).round() as u64)
}

/// The share of memory free, in percent: the kernel's own count of what is
/// available — `sysctl kern.memorystatus_level`, or `MemAvailable` of
/// `MemTotal` in `/proc/meminfo` where there is one. `None`: it cannot be
/// read.
pub fn memory_free() -> Option<u64> {
    if let Ok(said) = std::fs::read_to_string("/proc/meminfo") {
        return meminfo(&said);
    }
    sysctl("kern.memorystatus_level")?.trim().parse().ok()
}

fn meminfo(said: &str) -> Option<u64> {
    let field = |name: &str| -> Option<u64> {
        let line = said.lines().find(|line| line.starts_with(name))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    let (available, total) = (field("MemAvailable:")?, field("MemTotal:")?);
    (total > 0).then(|| available.saturating_mul(100) / total)
}

/// What `sysctl -n <name>` says, where it succeeds. It is run by name, as the
/// `PATH` finds it: a test world stands in its own.
fn sysctl(name: &str) -> Option<String> {
    let out = Command::new("sysctl")
        .args(["-n", name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Now, in epoch seconds.
#[cfg(test)]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_load_and_memory_are_read_as_sysctl_and_proc_say_them() {
        assert_eq!(first_load("{ 105.43 84.89 85.03 }\n"), Some(10543));
        assert_eq!(first_load("0.52 0.58 0.59 1/467 12345\n"), Some(52));
        assert_eq!(first_load("nothing"), None);
        let said = "MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    4000000 kB\n";
        assert_eq!(meminfo(said), Some(25));
        assert_eq!(meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
    }

    #[test]
    fn this_host_is_read() {
        let now = now();
        let reading = reading(&mut Minute::default(), now, 101).expect("this host is read");
        assert!(reading.busy <= 100, "{reading:?}");
        assert_eq!(reading.taken, now);
        let memory = reading.memory.expect("all memory free is under a floor past 100%");
        assert!(memory.free <= 100 && memory.floor == 101, "{memory:?}");
        let unfloored = super::reading(&mut Minute::default(), now, 0).unwrap();
        assert_eq!(unfloored.memory, None, "nothing is under no floor");
    }
}

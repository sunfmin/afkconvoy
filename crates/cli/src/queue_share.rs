//! The exclusive queue's share of the host's CPU, averaged over the minute
//! the load average is (docs/adr/0078, sunfmin/control#1069). Admission
//! apportions the host's one-minute load by it: the load lags the work that
//! made it by about a minute, and one instant's share does not, so a gate
//! between two of its phases left its whole lagging load counted as other
//! load. The host runner notes the share every few seconds while a command
//! of the queue runs, and each time it is asked what its fleets share; once
//! none runs, the average decays towards none as the load does.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use afkconvoy_core::exclusive::Ticket;

/// The span the share is averaged over: the load average's.
pub const SPAN: Duration = Duration::from_secs(60);

/// How often the host runner notes the share while the queue runs, as the
/// kernel samples the load.
pub const NOTED_EVERY: Duration = Duration::from_secs(5);

/// The share is said in ten-thousandths: all of the host's CPU is this.
pub const WHOLE: u64 = 10_000;

/// The host's daemons that scan what a gate starts and writes: each new
/// program is checked (`syspolicyd`, `XProtectService`, `opendirectoryd`)
/// and each file written is seen (`fseventsd`). The CPU they take while the
/// queue runs is the queue's: a gate's tests start dozens of programs a
/// second, and nothing else on the host makes them busy so.
pub const SCANNERS: [&str; 4] = ["fseventsd", "XProtectService", "syspolicyd", "opendirectoryd"];

/// What one reading saw of the queue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seen {
    /// None of its commands runs.
    Idle,
    /// Its running work, with the scanners, took this part of the host's
    /// CPU, from 0 to 1.
    Share(f64),
}

/// The average share, decayed with the span as the load average is.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Average {
    /// The second of the last reading noted, and the average then; `None`:
    /// the queue has run for none of the last span or so.
    noted: Option<(u64, f64)>,
}

impl Average {
    /// Notes what was `seen` at the second `at`. Its first reading of a
    /// running queue is the average; one that starts no average while none
    /// runs, and one in the same second as the last, changes nothing.
    pub fn note(&mut self, at: u64, seen: Seen) {
        let now = match seen {
            Seen::Idle => 0.0,
            Seen::Share(share) => share.clamp(0.0, 1.0),
        };
        self.noted = match (self.noted, seen) {
            (None, Seen::Idle) => None,
            (None, Seen::Share(_)) => Some((at, now)),
            (Some((last, average)), _) if at <= last => Some((last, average)),
            (Some((last, average)), _) => {
                let kept = (-((at - last) as f64) / SPAN.as_secs_f64()).exp();
                let average = average * kept + now * (1.0 - kept);
                (average >= 1.0 / WHOLE as f64 || seen != Seen::Idle).then_some((at, average))
            }
        };
    }

    /// The average, in ten-thousandths of the host's CPU; `None`: none.
    pub fn share(&self) -> Option<u64> {
        self.noted.map(|(_, average)| (average * WHOLE as f64).round() as u64)
    }
}

/// The part of the CPU of `procs`, one reading of the process table, that
/// the queue's running work `work` and the scanners take; `None`: the
/// reading says no process takes any.
pub(crate) fn share_of(
    procs: &[crate::hot::Proc],
    work: &std::collections::BTreeSet<u32>,
) -> Option<f64> {
    let total: u64 = procs.iter().map(|proc| proc.cpu_share).sum();
    let ours = |proc: &&crate::hot::Proc| work.contains(&proc.pid) || scanner(&proc.line);
    let queued: u64 = procs.iter().filter(ours).map(|proc| proc.cpu_share).sum();
    (total > 0).then(|| queued as f64 / total as f64)
}

/// Whether the program `comm`, as `ps` names it, is one of the scanners.
fn scanner(comm: &str) -> bool {
    let name = Path::new(comm.trim()).file_name().and_then(|name| name.to_str());
    name.is_some_and(|name| SCANNERS.contains(&name))
}

/// The average the host runner keeps; a clone is the same.
#[derive(Debug, Clone, Default)]
pub struct Shared(Arc<Mutex<Average>>);

impl Shared {
    /// Notes the share the queue `queue` takes now. A process table that
    /// cannot be read, or holds none of a running ticket's processes, or
    /// says no process takes any CPU, notes nothing.
    pub fn note(&self, queue: &[Ticket]) {
        let at = crate::clock();
        if self.average().noted.is_some_and(|(last, _)| at <= last) {
            return;
        }
        let seen = if queue.iter().any(Ticket::running) {
            let Ok(procs) = crate::hot::cpu_table() else { return };
            let work = crate::exclusive::running_work(queue, &procs);
            match share_of(&procs, &work.pids).filter(|_| work.whole) {
                Some(share) => Seen::Share(share),
                None => return,
            }
        } else {
            Seen::Idle
        };
        self.average().note(at, seen);
    }

    fn average(&self) -> std::sync::MutexGuard<'_, Average> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The average now, in ten-thousandths of the host's CPU; `None`: the
    /// queue has not run lately.
    pub fn share(&self) -> Option<u64> {
        self.average().share()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_794_705_373;

    fn proc(pid: u32, ppid: u32, percent: u64, comm: &str) -> crate::hot::Proc {
        crate::hot::Proc {
            pid,
            ppid,
            resident: 0,
            cpu_ms: 0,
            cpu_share: percent * 100,
            written: 0,
            line: comm.into(),
            cwd: None,
        }
    }

    #[test]
    fn a_queue_busy_over_the_last_minute_and_idle_now_keeps_most_of_its_share() {
        let mut average = Average::default();
        for second in (0..60).step_by(5) {
            average.note(AT + second, Seen::Share(0.9));
        }
        assert_eq!(average.share(), Some(9000));
        // Between two of a gate's phases its tree takes nothing for a moment.
        average.note(AT + 60, Seen::Share(0.0));
        let share = average.share().unwrap();
        assert!((8000..9000).contains(&share), "{share}");
        // And once the queue has ended, its share decays as the load does.
        average.note(AT + 120, Seen::Idle);
        let share = average.share().unwrap();
        assert!((2900..3400).contains(&share), "{share}");
        average.note(AT + 1200, Seen::Idle);
        assert_eq!(average.share(), None);
    }

    #[test]
    fn a_queue_idle_throughout_has_none_and_one_whose_work_takes_no_cpu_has_none_of_it() {
        let mut average = Average::default();
        average.note(AT, Seen::Idle);
        assert_eq!(average.share(), None);
        for second in (5..60).step_by(5) {
            average.note(AT + second, Seen::Share(0.0));
        }
        assert_eq!(average.share(), Some(0));
    }

    #[test]
    fn a_reading_in_the_same_second_changes_nothing() {
        let mut average = Average::default();
        average.note(AT, Seen::Share(1.0));
        average.note(AT, Seen::Share(0.0));
        assert_eq!(average.share(), Some(WHOLE));
    }

    #[test]
    fn the_scanners_cpu_is_the_queues_and_everything_else_is_others() {
        let procs = [
            proc(10, 1, 100, "afkconvoy"),
            proc(11, 10, 100, "cargo-nextest"),
            proc(20, 1, 100, "/usr/libexec/syspolicyd"),
            proc(21, 1, 100, "/System/Library/CoreServices/fseventsd"),
            proc(30, 1, 200, "/Applications/Someone Else.app/Contents/MacOS/Someone Else"),
        ];
        let work = [10, 11].into_iter().collect();
        assert_eq!(share_of(&procs, &work), Some(4.0 / 6.0));
        let idle = [proc(10, 1, 0, "afkconvoy")];
        assert_eq!(share_of(&idle, &work), None);
    }
}

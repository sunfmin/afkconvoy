//! A hot worker: one whose processes — the process tree under its terminal —
//! take more than their share of the host's CPU or memory (docs/adr/0019).
//! When a worker turns hot, and when it stops being one, is decided here from
//! the samples `run` takes of it; reading the processes and the host is the
//! effect layer's.

use serde::{Deserialize, Serialize};

/// How often, in seconds, `run` samples each worker's processes.
pub const SAMPLE_EVERY: u64 = 10;

/// How long, in seconds, a worker's processes must stay past the CPU limit
/// for it to turn hot: two minutes.
pub const CPU_HELD: u64 = 120;

/// How long, in seconds, a hot worker must stay under every limit for it to
/// stop being hot: five minutes. That ends its episode.
pub const COOLS_AFTER: u64 = 300;

/// What a worker's processes may take of the host before it turns hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// CPU, in thousandths of a core: more than this, held `CPU_HELD`.
    pub millicores: u64,
    /// Resident memory, in bytes: more than this.
    pub resident: u64,
}

impl Limits {
    /// The limits on a host of `cores` cores and `memory` bytes of memory,
    /// each a share of it: half its cores, but never under one, and a quarter
    /// of its memory.
    pub fn of_host(cores: u64, memory: u64) -> Limits {
        Limits { millicores: cores.saturating_mul(500).max(1000), resident: memory / 4 }
    }
}

/// The thousandths of a core a process tree took, having taken `cpu_ms`
/// milliseconds of CPU in `wall_ms` milliseconds; none in no time at all.
pub fn millicores(cpu_ms: u64, wall_ms: u64) -> u64 {
    if wall_ms == 0 { 0 } else { cpu_ms.saturating_mul(1000) / wall_ms }
}

/// One sample of one worker's processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// When it was taken, in epoch seconds.
    pub at: u64,
    /// The thousandths of a core they took since the sample before; `None`
    /// at the first, which has no sample before it.
    pub millicores: Option<u64>,
    /// The bytes they held resident.
    pub resident: u64,
    /// Whether the host was under memory pressure while they held the most
    /// of every worker sampled with them.
    pub most_under_pressure: bool,
}

/// Why a worker turned hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// Its processes took more CPU than the limit, for `CPU_HELD`.
    Cpu,
    /// They held more memory resident than the limit.
    Memory,
    /// The host was under memory pressure, and they held the most.
    Pressure,
}

/// A worker's episode of being hot: since when, why, and the most it took
/// while it lasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub since: u64,
    pub why: Why,
    /// The most CPU taken in one sample, in thousandths of a core.
    pub millicores: u64,
    /// The most memory held resident in one sample, in bytes.
    pub resident: u64,
}

/// A worker turning hot, or stopping being one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turned {
    Hot(Episode),
    /// Its episode ended, having lasted until `at`.
    Cooled {
        episode: Episode,
        at: u64,
    },
}

/// What is known of one worker from its samples so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Watch {
    /// Since when its CPU has been past the limit, sample after sample.
    cpu_since: Option<u64>,
    /// Its episode, while it is hot.
    pub hot: Option<Episode>,
    /// Since when, hot, it has been under every limit.
    under_since: Option<u64>,
}

impl Watch {
    /// Takes in `sample`, under `limits`, `every` seconds after the sample
    /// before → whether the worker turned hot with it, or stopped being hot.
    ///
    /// It turns hot when its CPU has been past the limit for `CPU_HELD`,
    /// counted from the start of the first sample past it; when it holds more
    /// memory than the limit; or when the host is under memory pressure and it
    /// holds the most. Hot, it is hot once only: it stops being hot once it has
    /// been under every limit for `COOLS_AFTER`, and hot again after that is a
    /// new episode.
    pub fn sampled(&mut self, sample: &Sample, limits: &Limits, every: u64) -> Option<Turned> {
        let cpu_over = sample.millicores.is_some_and(|taken| taken > limits.millicores);
        self.cpu_since = match self.cpu_since {
            Some(since) if cpu_over => Some(since),
            None if cpu_over => Some(sample.at.saturating_sub(every)),
            _ => None,
        };
        let cpu_held =
            self.cpu_since.is_some_and(|since| sample.at.saturating_sub(since) >= CPU_HELD);
        let memory_over = sample.resident > limits.resident;
        let why = if cpu_held {
            Some(Why::Cpu)
        } else if memory_over {
            Some(Why::Memory)
        } else if sample.most_under_pressure {
            Some(Why::Pressure)
        } else {
            None
        };
        let taken = sample.millicores.unwrap_or(0);
        let Some(mut episode) = self.hot else {
            let why = why?;
            let started =
                Episode { since: sample.at, why, millicores: taken, resident: sample.resident };
            self.hot = Some(started);
            self.under_since = None;
            return Some(Turned::Hot(started));
        };
        episode.millicores = episode.millicores.max(taken);
        episode.resident = episode.resident.max(sample.resident);
        self.hot = Some(episode);
        let under = !cpu_over && !memory_over && !sample.most_under_pressure;
        if !under {
            self.under_since = None;
            return None;
        }
        let under_since = *self.under_since.get_or_insert(sample.at);
        if sample.at.saturating_sub(under_since) < COOLS_AFTER {
            return None;
        }
        self.hot = None;
        self.under_since = None;
        Some(Turned::Cooled { episode, at: sample.at })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;
    const LIMITS: Limits = Limits { millicores: 4000, resident: 8 * GIB };

    fn sample(at: u64, millicores: u64, resident: u64) -> Sample {
        Sample { at, millicores: Some(millicores), resident, most_under_pressure: false }
    }

    /// Samples one every `SAMPLE_EVERY` seconds from `from`, each taking
    /// `millicores` and holding `resident` → what each turned, by its time.
    fn run(
        watch: &mut Watch,
        from: u64,
        until: u64,
        millicores: u64,
        resident: u64,
    ) -> Vec<(u64, Turned)> {
        let mut turned = vec![];
        let mut at = from;
        while at <= until {
            if let Some(turn) =
                watch.sampled(&sample(at, millicores, resident), &LIMITS, SAMPLE_EVERY)
            {
                turned.push((at, turn));
            }
            at += SAMPLE_EVERY;
        }
        turned
    }

    #[test]
    fn the_limits_are_shares_of_the_host() {
        assert_eq!(Limits::of_host(14, 48 * GIB), Limits { millicores: 7000, resident: 12 * GIB });
        // one core at the least, on a host of one
        assert_eq!(Limits::of_host(1, 4 * GIB).millicores, 1000);
        assert_eq!(Limits::of_host(0, 0), Limits { millicores: 1000, resident: 0 });
    }

    #[test]
    fn cores_are_the_cpu_taken_over_the_time_it_was_taken_in() {
        assert_eq!(millicores(20_000, 10_000), 2000);
        assert_eq!(millicores(500, 10_000), 50);
        assert_eq!(millicores(500, 0), 0);
    }

    #[test]
    fn a_worker_past_the_cpu_limit_for_two_minutes_turns_hot_and_one_under_it_never() {
        let mut busy = Watch::default();
        let turned = run(&mut busy, 1000, 1300, 6000, GIB);
        // past it from 990, the start of the first sample: hot at 990 + 120
        assert_eq!(turned.len(), 1, "{turned:?}");
        let (at, Turned::Hot(episode)) = turned[0] else { panic!("{turned:?}") };
        assert_eq!(at, 1110);
        assert_eq!(
            episode,
            Episode { since: 1110, why: Why::Cpu, millicores: 6000, resident: GIB }
        );

        let mut under = Watch::default();
        assert!(run(&mut under, 1000, 5000, 4000, GIB).is_empty(), "at the limit is not past it");
    }

    #[test]
    fn cpu_past_the_limit_must_be_held_unbroken() {
        let mut watch = Watch::default();
        assert!(run(&mut watch, 1000, 1100, 6000, GIB).is_empty());
        // one sample under starts the two minutes over
        assert!(run(&mut watch, 1110, 1110, 100, GIB).is_empty());
        let turned = run(&mut watch, 1120, 1300, 6000, GIB);
        assert_eq!(turned.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [1230]);
    }

    #[test]
    fn the_first_sample_takes_no_cpu() {
        let mut watch = Watch::default();
        let first =
            Sample { at: 1000, millicores: None, resident: GIB, most_under_pressure: false };
        assert_eq!(watch.sampled(&first, &LIMITS, SAMPLE_EVERY), None);
        assert_eq!(watch, Watch::default());
    }

    #[test]
    fn a_worker_past_the_memory_limit_turns_hot_at_once() {
        let mut watch = Watch::default();
        let turned = run(&mut watch, 1000, 1000, 100, 9 * GIB);
        assert_eq!(
            turned,
            [(
                1000,
                Turned::Hot(Episode {
                    since: 1000,
                    why: Why::Memory,
                    millicores: 100,
                    resident: 9 * GIB
                })
            )]
        );
    }

    #[test]
    fn under_pressure_the_worker_that_holds_the_most_turns_hot() {
        let mut watch = Watch::default();
        let most = Sample { most_under_pressure: true, ..sample(1000, 100, GIB) };
        let Some(Turned::Hot(episode)) = watch.sampled(&most, &LIMITS, SAMPLE_EVERY) else {
            panic!("{watch:?}")
        };
        assert_eq!(episode.why, Why::Pressure);
    }

    #[test]
    fn an_episode_ends_after_five_minutes_under_and_a_later_one_is_new() {
        let mut watch = Watch::default();
        assert_eq!(run(&mut watch, 1000, 1000, 100, 9 * GIB).len(), 1);
        // still hot: its peak grows, and nothing is turned
        assert!(run(&mut watch, 1010, 1100, 100, 10 * GIB).is_empty());
        // under from 1110: not yet at 1400, cooled at 1410
        let turned = run(&mut watch, 1110, 1500, 100, GIB);
        let episode =
            Episode { since: 1000, why: Why::Memory, millicores: 100, resident: 10 * GIB };
        assert_eq!(turned, [(1410, Turned::Cooled { episode, at: 1410 })]);
        assert_eq!(watch.hot, None);
        // hot again: a new episode, since then
        let again = run(&mut watch, 2000, 2000, 100, 9 * GIB);
        let Turned::Hot(episode) = again[0].1 else { panic!("{again:?}") };
        assert_eq!((episode.since, episode.resident), (2000, 9 * GIB));
    }

    #[test]
    fn a_hot_worker_over_again_before_five_minutes_starts_its_wait_over() {
        let mut watch = Watch::default();
        run(&mut watch, 1000, 1000, 100, 9 * GIB);
        assert!(run(&mut watch, 1010, 1200, 100, GIB).is_empty());
        // over once more: the five minutes under start again after it
        assert!(run(&mut watch, 1210, 1210, 6000, GIB).is_empty());
        let turned = run(&mut watch, 1220, 1600, 100, GIB);
        assert_eq!(turned.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [1520]);
    }

    #[test]
    fn cpu_past_the_limit_but_not_yet_held_keeps_a_worker_hot() {
        let mut watch = Watch::default();
        run(&mut watch, 1000, 1000, 100, 9 * GIB);
        // past the cpu limit, never for two minutes: never under, never cooled
        assert!(run(&mut watch, 1010, 2000, 6000, GIB).is_empty());
        assert!(watch.hot.is_some());
    }
}

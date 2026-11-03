//! A hot worker: one whose processes — the process tree under its terminal —
//! take more than their share of the host's CPU, memory or disk, or whose
//! worktree has outgrown its fresh build (docs/adr/0019). When a worker turns
//! hot, and when it stops being one, is decided here from the samples `run`
//! takes of it; reading the processes, the worktree and the host is the effect
//! layer's.

use serde::{Deserialize, Serialize};

use crate::landing::Built;

/// How often, in seconds, `run` samples each worker's processes.
pub const SAMPLE_EVERY: u64 = 10;

/// How long, in seconds, a worker's processes must stay past the CPU limit
/// for it to turn hot: two minutes.
pub const CPU_HELD: u64 = 120;

/// The window, in seconds, the bytes a worker's processes wrote are counted
/// over: ten minutes.
pub const WRITTEN_WITHIN: u64 = 600;

/// How long, in seconds, a hot worker must stay under every limit for it to
/// stop being hot: five minutes. That ends its episode.
pub const COOLS_AFTER: u64 = 300;

/// The least the disk-written limit is, in bytes, however little disk the
/// host has free: 1 GiB.
pub const WRITTEN_FLOOR: u64 = 1 << 30;

/// What a worker's processes may take of the host before it turns hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// CPU, in thousandths of a core: more than this, held `CPU_HELD`.
    pub millicores: u64,
    /// Resident memory, in bytes: more than this.
    pub resident: u64,
    /// Bytes written to disk: more than this within `WRITTEN_WITHIN`.
    pub written: u64,
}

impl Limits {
    /// The limits on a host of `cores` cores, `memory` bytes of memory and
    /// `free` bytes of disk free, each a share of it: half its cores, but
    /// never under one; a quarter of its memory; a tenth of its free disk,
    /// but never under `WRITTEN_FLOOR`.
    pub fn of_host(cores: u64, memory: u64, free: u64) -> Limits {
        Limits {
            millicores: cores.saturating_mul(500).max(1000),
            resident: memory / 4,
            written: (free / 10).max(WRITTEN_FLOOR),
        }
    }
}

/// The thousandths of a core a process tree took, having taken `cpu_ms`
/// milliseconds of CPU in `wall_ms` milliseconds; none in no time at all.
pub fn millicores(cpu_ms: u64, wall_ms: u64) -> u64 {
    cpu_ms.saturating_mul(1000).checked_div(wall_ms).unwrap_or(0)
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
    /// The bytes of `resident` held under an `afkconvoy exclusive` command:
    /// the exclusive queue bounds that heavy work, and it counts against the
    /// memory limit only while the host is `pressed`.
    pub exclusive: u64,
    /// Whether the host's memory pressure was at level 2 or above.
    pub pressed: bool,
    /// The bytes they wrote to disk since the sample before; `None` at the
    /// first.
    pub written: Option<u64>,
    /// What the worker's worktree holds, measured with this sample; `None`
    /// when it was not, for measuring it is slow: the measure before stands.
    pub held: Option<Built>,
}

impl Sample {
    /// The bytes resident that count against the memory limit: those outside
    /// its exclusive commands, or every one while the host is pressed.
    pub fn counted(&self) -> u64 {
        if self.pressed { self.resident } else { self.resident.saturating_sub(self.exclusive) }
    }
}

/// Why a worker turned hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// Its processes took more CPU than the limit, for `CPU_HELD`.
    Cpu,
    /// They held more memory resident than the limit.
    Memory,
    /// They wrote more to disk than the limit, within `WRITTEN_WITHIN`.
    Written,
    /// Its worktree has outgrown its fresh build (`landing::outgrown`).
    Held,
}

impl Why {
    /// Which of what is watched it is, as a person names it.
    pub fn word(self) -> &'static str {
        match self {
            Why::Cpu => "CPU",
            Why::Memory => "memory",
            Why::Written => "disk written",
            Why::Held => "disk held",
        }
    }
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
    /// The most written to disk within `WRITTEN_WITHIN`, in bytes.
    #[serde(default)]
    pub written: u64,
    /// The most its worktree held of what git ignores, in bytes.
    #[serde(default)]
    pub held: u64,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    /// Since when its CPU has been past the limit, sample after sample.
    cpu_since: Option<u64>,
    /// The bytes written in each sample within `WRITTEN_WITHIN`, by when it
    /// was taken.
    written: Vec<(u64, u64)>,
    /// What its worktree held, as last measured.
    held: Option<Built>,
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
    /// memory than the limit — outside its exclusive commands, but for while
    /// the host is pressed (`Sample::counted`); when it wrote more than the limit within
    /// `WRITTEN_WITHIN`, counted from the start of the oldest sample in it; or
    /// when its worktree, as last measured, has outgrown its fresh build
    /// (`Built::outgrown`). Hot, it is hot once only: it stops being hot once
    /// it has been under every limit for `COOLS_AFTER` — its worktree back
    /// under the rule among them, for files do not go when their process ends
    /// — and hot again after that is a new episode.
    pub fn sampled(&mut self, sample: &Sample, limits: &Limits, every: u64) -> Option<Turned> {
        let cpu_over = sample.millicores.is_some_and(|taken| taken > limits.millicores);
        self.cpu_since = match self.cpu_since {
            Some(since) if cpu_over => Some(since),
            None if cpu_over => Some(sample.at.saturating_sub(every)),
            _ => None,
        };
        let cpu_held =
            self.cpu_since.is_some_and(|since| sample.at.saturating_sub(since) >= CPU_HELD);
        let memory_over = sample.counted() > limits.resident;
        // A sample counts what was written from the one before it on.
        let within = sample.at.saturating_sub(WRITTEN_WITHIN.saturating_sub(every));
        self.written.retain(|(at, _)| *at >= within);
        if let Some(written) = sample.written {
            self.written.push((sample.at, written));
        }
        let written =
            self.written.iter().fold(0, |sum: u64, (_, bytes)| sum.saturating_add(*bytes));
        let written_over = written > limits.written;
        if sample.held.is_some() {
            self.held = sample.held;
        }
        let outgrown = self.held.is_some_and(Built::outgrown);
        let held = self.held.map_or(0, |held| held.built);
        let why = if cpu_held {
            Some(Why::Cpu)
        } else if memory_over {
            Some(Why::Memory)
        } else if written_over {
            Some(Why::Written)
        } else if outgrown {
            Some(Why::Held)
        } else {
            None
        };
        let taken = sample.millicores.unwrap_or(0);
        let Some(mut episode) = self.hot else {
            let why = why?;
            let started = Episode {
                since: sample.at,
                why,
                millicores: taken,
                resident: sample.resident,
                written,
                held,
            };
            self.hot = Some(started);
            self.under_since = None;
            return Some(Turned::Hot(started));
        };
        episode.millicores = episode.millicores.max(taken);
        episode.resident = episode.resident.max(sample.resident);
        episode.written = episode.written.max(written);
        episode.held = episode.held.max(held);
        self.hot = Some(episode);
        let under = !cpu_over && !memory_over && !written_over && !outgrown;
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
    const LIMITS: Limits = Limits { millicores: 4000, resident: 8 * GIB, written: 4 * GIB };

    fn sample(at: u64, millicores: u64, resident: u64) -> Sample {
        Sample {
            at,
            millicores: Some(millicores),
            resident,
            exclusive: 0,
            pressed: false,
            written: Some(0),
            held: None,
        }
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
        assert_eq!(
            Limits::of_host(14, 48 * GIB, 200 * GIB),
            Limits { millicores: 7000, resident: 12 * GIB, written: 20 * GIB }
        );
        // one core at the least, on a host of one
        assert_eq!(Limits::of_host(1, 4 * GIB, 0).millicores, 1000);
        // a GiB written at the least, on a host with its disk full
        assert_eq!(
            Limits::of_host(0, 0, 0),
            Limits { millicores: 1000, resident: 0, written: WRITTEN_FLOOR }
        );
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
            Episode {
                since: 1110,
                why: Why::Cpu,
                millicores: 6000,
                resident: GIB,
                written: 0,
                held: 0
            }
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
        let first = Sample {
            at: 1000,
            millicores: None,
            resident: GIB,
            exclusive: 0,
            pressed: false,
            written: None,
            held: None,
        };
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
                    resident: 9 * GIB,
                    written: 0,
                    held: 0
                })
            )]
        );
    }

    #[test]
    fn host_pressure_never_makes_a_worker_under_its_memory_limit_hot() {
        use crate::resources::{Holding, Reason, SPAN};

        let mut host = Holding::default();
        let mut worker = Watch::default();
        for at in (1000..=1600).step_by(SAMPLE_EVERY as usize) {
            host.ticked(at, &[Reason::Memory]);
            assert_eq!(worker.sampled(&sample(at, 100, GIB), &LIMITS, SAMPLE_EVERY), None);
            assert_eq!(worker.hot, None);
        }
        assert_eq!(host.due(1600, SPAN).map(|due| due.reason), Some(Reason::Memory));
        // Its own memory limit still applies under host pressure.
        assert!(matches!(
            worker.sampled(&sample(1610, 100, 9 * GIB), &LIMITS, SAMPLE_EVERY),
            Some(Turned::Hot(Episode { why: Why::Memory, .. }))
        ));
    }

    #[test]
    fn memory_under_an_exclusive_command_makes_a_worker_hot_only_under_pressure() {
        let under = |exclusive, pressed| Sample { exclusive, pressed, ..sample(1000, 100, 9 * GIB) };
        let turned = |sample: Sample| Watch::default().sampled(&sample, &LIMITS, SAMPLE_EVERY);
        // Over the limit only through what runs under `exclusive`, pressure low.
        assert_eq!(turned(under(2 * GIB, false)), None);
        // The same worker with the host pressed.
        assert!(matches!(
            turned(under(2 * GIB, true)),
            Some(Turned::Hot(Episode { why: Why::Memory, resident, .. })) if resident == 9 * GIB
        ));
        // Over the limit outside `exclusive`, pressure low.
        assert!(matches!(
            turned(under(GIB / 2, false)),
            Some(Turned::Hot(Episode { why: Why::Memory, .. }))
        ));
        // Hot, it cools once what counts is back under, though its exclusive
        // work still holds the memory.
        let mut watch = Watch::default();
        assert!(watch.sampled(&sample(1000, 100, 9 * GIB), &LIMITS, SAMPLE_EVERY).is_some());
        let mut cooled = None;
        for at in (1010..=1400).step_by(SAMPLE_EVERY as usize) {
            let sample = Sample { exclusive: 2 * GIB, ..sample(at, 100, 9 * GIB) };
            cooled = cooled.or(watch.sampled(&sample, &LIMITS, SAMPLE_EVERY));
        }
        assert!(matches!(cooled, Some(Turned::Cooled { at: 1310, .. })), "{cooled:?}");
    }

    #[test]
    fn an_episode_ends_after_five_minutes_under_and_a_later_one_is_new() {
        let mut watch = Watch::default();
        assert_eq!(run(&mut watch, 1000, 1000, 100, 9 * GIB).len(), 1);
        // still hot: its peak grows, and nothing is turned
        assert!(run(&mut watch, 1010, 1100, 100, 10 * GIB).is_empty());
        // under from 1110: not yet at 1400, cooled at 1410
        let turned = run(&mut watch, 1110, 1500, 100, GIB);
        let episode = Episode {
            since: 1000,
            why: Why::Memory,
            millicores: 100,
            resident: 10 * GIB,
            written: 0,
            held: 0,
        };
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

    /// Samples one every `SAMPLE_EVERY` seconds from `from`, each idle but for
    /// writing `written` bytes and, where `held` says, its worktree measured.
    fn run_disk(
        watch: &mut Watch,
        from: u64,
        until: u64,
        written: u64,
        held: Option<Built>,
    ) -> Vec<(u64, Turned)> {
        let mut turned = vec![];
        let mut at = from;
        while at <= until {
            let sample = Sample { written: Some(written), held, ..sample(at, 100, GIB) };
            if let Some(turn) = watch.sampled(&sample, &LIMITS, SAMPLE_EVERY) {
                turned.push((at, turn));
            }
            at += SAMPLE_EVERY;
        }
        turned
    }

    #[test]
    fn a_worker_writing_past_the_limit_within_ten_minutes_turns_hot_as_it_does() {
        let mut watch = Watch::default();
        // 100 MiB a sample: 4 GiB is passed with the 41st, at 1400
        let turned = run_disk(&mut watch, 1000, 1500, 100 << 20, None);
        assert_eq!(turned.len(), 1, "{turned:?}");
        let (at, Turned::Hot(episode)) = turned[0] else { panic!("{turned:?}") };
        assert_eq!((at, episode.why, episode.written), (1400, Why::Written, 41 * (100 << 20)));

        // as much, but spread past ten minutes: never past the limit within them
        let mut slow = Watch::default();
        assert!(run_disk(&mut slow, 1000, 5000, 60 << 20, None).is_empty());
    }

    #[test]
    fn a_worker_that_stops_writing_cools_as_cpu_and_memory_do() {
        let mut watch = Watch::default();
        let hot = run_disk(&mut watch, 1000, 1000, 5 * GIB, None);
        assert_eq!(hot.len(), 1);
        // what it wrote stays in the window until ten minutes have passed,
        // 1590; under from 1600, it cools five minutes after, at 1900
        let turned = run_disk(&mut watch, 1010, 2500, 0, None);
        assert_eq!(turned.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [1900]);
        let Turned::Cooled { episode, .. } = turned[0].1 else { panic!("{turned:?}") };
        assert_eq!((episode.why, episode.written), (Why::Written, 5 * GIB));
    }

    #[test]
    fn a_worktree_that_outgrew_its_fresh_build_is_hot_until_it_is_back_under_the_rule() {
        let mut watch = Watch::default();
        let fresh = Some(2 * GIB);
        let within = Built { built: 6 * GIB, fresh };
        assert!(run_disk(&mut watch, 1000, 1000, 0, Some(within)).is_empty());
        let outgrown = Built { built: 7 * GIB, fresh };
        let turned = run_disk(&mut watch, 1010, 1010, 0, Some(outgrown));
        let Turned::Hot(episode) = turned[0].1 else { panic!("{turned:?}") };
        assert_eq!((episode.why, episode.held), (Why::Held, 7 * GIB));
        // its processes idle, its worktree not measured again: it stays hot
        assert!(run_disk(&mut watch, 1020, 5000, 0, None).is_empty());
        assert!(watch.hot.is_some());
        // measured back under the rule: five minutes on, it cools
        let back = Some(Built { built: 3 * GIB, fresh });
        assert!(run_disk(&mut watch, 5010, 5010, 0, back).is_empty());
        let turned = run_disk(&mut watch, 5020, 5400, 0, None);
        assert_eq!(turned.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [5310]);
    }
}

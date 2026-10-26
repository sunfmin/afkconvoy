//! What a `run` does with the host runner it finds, by the release each is
//! of: a newer afkconvoy moves the host runner onto itself, and an older one
//! changes nothing; what holds back the host runner's own move onto a newer
//! afkconvoy installed where its binary was; and how long a broken fleet
//! instance waits before the host runner starts it again (docs/adr/0023).

use flux_rs::attrs::*;

/// How long a fleet instance that broke for the first time waits before the
/// host runner starts it again, in seconds: a minute.
pub const BACKOFF_FIRST: u64 = 60;

/// The longest a broken fleet instance waits before it is started again, in
/// seconds: half an hour. One that ran this long since its last start broke
/// anew, and waits `BACKOFF_FIRST`.
pub const BACKOFF_MOST: u64 = 1800;

/// How long a fleet instance that broke waits before the host runner starts
/// it again, in seconds, having waited `waited` before its last start —
/// `None` where it had not broken — and run `up` seconds since that start:
/// twice what it waited, within `BACKOFF_FIRST` and `BACKOFF_MOST`. One that
/// had not broken, or ran `BACKOFF_MOST` or longer, waits `BACKOFF_FIRST`: a
/// minute, then 2, 4, 8, 16 and 30 minutes for each break in a row.
#[spec(fn(waited: Option<u64>, up: u64) -> u64{wait: BACKOFF_FIRST <= wait && wait <= BACKOFF_MOST})]
pub fn backoff(waited: Option<u64>, up: u64) -> u64 {
    match waited {
        Some(waited) if up < BACKOFF_MOST => {
            let doubled = if waited > BACKOFF_MOST / 2 { BACKOFF_MOST } else { 2 * waited };
            if doubled < BACKOFF_FIRST { BACKOFF_FIRST } else { doubled }
        }
        _ => BACKOFF_FIRST,
    }
}

/// The host runner a `run` found, against the `run`'s own release.
#[derive(Debug, PartialEq, Eq)]
pub enum Found {
    /// Of the same release: it is handed the checkout, as ever.
    Same,
    /// Of an older release: it is stopped, and one of the `run`'s own started
    /// on the same checkout list.
    Older,
    /// Of a newer release: the `run` changes nothing.
    Newer,
}

/// The host runner of the afkconvoy `theirs`, as a `run` of the afkconvoy
/// `ours` finds it, each version as `--version` says it: a release
/// (`0.6.0`), or a build past one (`0.6.0+11.g9f58035`), which is of that
/// release. Releases are compared number by number. A version whose release
/// is not all numbers is known to be neither older nor newer: `Same`.
pub fn found(ours: &str, theirs: &str) -> Found {
    match (release(ours), release(theirs)) {
        (Some(ours), Some(theirs)) if theirs < ours => Found::Older,
        (Some(ours), Some(theirs)) if theirs > ours => Found::Newer,
        _ => Found::Same,
    }
}

/// The release `version` is of, number by number: `0.6.0+11.g9f58035` is
/// [0, 6, 0]. `None` where a part of it is no number.
fn release(version: &str) -> Option<Vec<u64>> {
    let release = version.split('+').next().unwrap_or(version);
    release.split('.').map(|number| number.parse().ok()).collect()
}

/// How one fleet instance of a host runner stands, as the host runner's move
/// onto a newer afkconvoy reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stands {
    /// Started, and its first tick has not ended yet.
    Starting,
    /// In a tick.
    Ticking,
    /// Between two ticks, the last of which held a claim — on an issue, a
    /// patrol's run or a proposal — or had something on the landing train,
    /// or failed whole.
    AtWork,
    /// Between two ticks, the last of which held no claim and had nothing on
    /// the landing train: a frontier it left is the next tick's.
    Quiet,
    /// Broken, and waiting to be started again: the host runner it moves
    /// onto starts it.
    Broken,
}

/// One thing that holds back a host runner's move onto a newer afkconvoy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeldBack {
    /// The fleet instance of the checkout `checkout` stands so.
    Instance { checkout: String, stands: Stands },
    /// This many exclusive commands are running. Waiting commands carry on
    /// in the next host runner's queue.
    Exclusive { commands: usize },
}

impl HeldBack {
    /// The thing, as the host runner's log and `doctor` say it.
    pub fn said(&self) -> String {
        match self {
            HeldBack::Instance { checkout, stands: Stands::Starting } => {
                format!("{checkout} has not ended its first tick")
            }
            HeldBack::Instance { checkout, stands: Stands::Ticking } => {
                format!("{checkout} is in a tick")
            }
            HeldBack::Instance { checkout, stands: Stands::AtWork } => {
                format!("{checkout} holds a claim or has something on the landing train")
            }
            HeldBack::Instance { checkout, stands: Stands::Quiet | Stands::Broken } => {
                format!("{checkout} holds nothing back")
            }
            HeldBack::Exclusive { commands } => {
                format!("{commands} exclusive command(s) running")
            }
        }
    }
}

/// What holds back a host runner's move onto a newer afkconvoy now: each of
/// its fleet instances `each`, by checkout, still starting or in a tick,
/// and the `exclusive` commands running. Claims, workers, the landing train,
/// cuts, inspections and commands waiting carry on across a move.
pub fn held_back(each: &[(String, Stands)], exclusive: usize) -> Vec<HeldBack> {
    let mut held = vec![];
    for (checkout, stands) in each {
        if matches!(stands, Stands::Starting | Stands::Ticking) {
            held.push(HeldBack::Instance { checkout: checkout.clone(), stands: *stands });
        }
    }
    if exclusive > 0 {
        held.push(HeldBack::Exclusive { commands: exclusive });
    }
    held
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn a_fleet_instance_that_breaks_in_a_row_waits_twice_as_long_up_to_half_an_hour() {
        let mut waited = None;
        let mut waits = vec![];
        for _ in 0..8 {
            let wait = backoff(waited, 5);
            waits.push(wait / 60);
            waited = Some(wait);
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn one_that_ran_half_an_hour_before_it_broke_broke_anew() {
        assert_eq!(backoff(Some(BACKOFF_MOST), BACKOFF_MOST), BACKOFF_FIRST);
        assert_eq!(backoff(Some(BACKOFF_MOST), BACKOFF_MOST - 1), BACKOFF_MOST);
        assert_eq!(backoff(Some(480), 86_400), BACKOFF_FIRST);
        assert_eq!(backoff(None, 0), BACKOFF_FIRST, "its first break");
    }

    proptest! {
        #[test]
        fn a_wait_is_within_its_bounds_and_never_shorter_than_the_one_before_in_a_row(
            waited in proptest::option::of(any::<u64>()),
            up in any::<u64>(),
        ) {
            let wait = backoff(waited, up);
            prop_assert!((BACKOFF_FIRST..=BACKOFF_MOST).contains(&wait));
            if let Some(waited) = waited.filter(|_| up < BACKOFF_MOST) {
                prop_assert!(wait >= waited.min(BACKOFF_MOST));
            }
        }
    }

    #[test]
    fn a_host_runner_of_an_older_release_is_older_and_of_a_newer_newer() {
        assert_eq!(found("0.6.0", "0.5.13"), Found::Older);
        assert_eq!(found("0.5.13", "0.6.0"), Found::Newer);
        // number by number, never as text
        assert_eq!(found("0.5.13", "0.5.9"), Found::Older);
        assert_eq!(found("0.10.0", "0.9.99"), Found::Older);
        assert_eq!(found("1.0.0", "0.99.99"), Found::Older);
        assert_eq!(found("0.5.9", "0.5.13"), Found::Newer);
        // a build past a release is of that release
        assert_eq!(found("0.6.0+3.g9f58035", "0.5.13+11.gabcdef0.dirty"), Found::Older);
        assert_eq!(found("0.5.13", "0.6.0+1.g9f58035"), Found::Newer);
    }

    #[test]
    fn a_host_runner_moves_between_ticks_with_work_held_but_no_exclusive_command_running() {
        let each = |stands: &[Stands]| -> Vec<(String, Stands)> {
            stands.iter().enumerate().map(|(at, stands)| (format!("/c{at}"), *stands)).collect()
        };
        // a frontier left, or a fleet instance broken, holds nothing back
        assert_eq!(held_back(&each(&[Stands::Quiet, Stands::Broken, Stands::AtWork]), 0), vec![]);
        assert_eq!(held_back(&[], 0), vec![], "no fleet instance at all");
        for stands in [Stands::Starting, Stands::Ticking] {
            let held = held_back(&each(&[Stands::Quiet, stands]), 0);
            let instance = HeldBack::Instance { checkout: "/c1".to_string(), stands };
            assert_eq!(held, vec![instance], "{stands:?}");
        }
        let held = held_back(&each(&[Stands::Ticking, Stands::AtWork]), 1);
        let said: Vec<String> = held.iter().map(HeldBack::said).collect();
        assert_eq!(said, ["/c0 is in a tick", "1 exclusive command(s) running"]);
        assert_eq!(held_back(&each(&[Stands::Quiet]), 1).len(), 1, "a command alone");
    }

    #[test]
    fn a_build_of_the_same_release_is_the_same_and_so_is_one_of_no_release() {
        assert_eq!(found("0.6.0", "0.6.0"), Found::Same);
        assert_eq!(found("0.6.0+11.g9f58035", "0.6.0"), Found::Same);
        assert_eq!(found("0.6.0", "0.6.0+2.g1234567.dirty"), Found::Same);
        for unknown in ["", "unknown", "0.6.x", "0.6.0-rc.1"] {
            assert_eq!(found("0.6.0", unknown), Found::Same, "{unknown:?}");
            assert_eq!(found(unknown, "0.6.0"), Found::Same, "{unknown:?}");
        }
    }
}

//! Host sleep: a fleet instance putting its host to sleep once every fleet
//! instance on it is idle, within the host's sleep hours (docs/adr/0016).
//! What keeps the host awake is decided here, from what each instance
//! answered, how long the keyboard and mouse have gone untouched, when the
//! host last woke and its local time; asking the instances, reading the host
//! and putting it to sleep are the effect layer's.

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

/// How long, in seconds, every instance must have been idle — and the
/// keyboard and mouse untouched — before the host is put to sleep: ten
/// minutes. It is also how long a check that kept the host awake waits before
/// the next.
pub const IDLE_WAIT: u64 = 600;

/// How long, in seconds, an instance is given to answer.
pub const ANSWERS_WITHIN: u64 = 2;

/// How long, in seconds, an instance asked to tick first is given to answer.
pub const TICKS_WITHIN: u64 = 120;

/// The minutes of a day.
pub const DAY: u32 = 24 * 60;

/// The seconds of a day.
const DAY_SECONDS: u64 = DAY as u64 * 60;

/// A host's sleep hours: the hours of its day, in its local time, a fleet may
/// put it to sleep in — from `start`, included, to `end`, not, each in minutes
/// past its local midnight, and past midnight where `end` comes first. They
/// cover some of the day: `00:00-24:00` is all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(start: int, end: int)]
#[invariant(start < DAY && end <= DAY && start != end)]
pub struct Hours {
    #[field(u32[start])]
    start: u32,
    #[field(u32[end])]
    end: u32,
}

/// The sleep hours of a host that says none: from midnight to six.
pub const SLEEP_HOURS: Hours = Hours { start: 0, end: 6 * 60 };

/// What sleep hours are said as, for whoever said them wrong.
const SAID_AS: &str = "say them as HH:MM-HH:MM, the host's local time from 00:00 to 24:00, the \
                       start included and the end not: 00:00-06:00, 22:00-06:00 past midnight, \
                       or 00:00-24:00 for any hour";

impl Hours {
    /// Sleep hours as said, `HH:MM-HH:MM` → them; or, where they cannot be
    /// read or cover no time of the day, what is wrong, naming what they take.
    pub fn parse(said: &str) -> Result<Hours, String> {
        let times = said.split_once('-').and_then(|(start, end)| Some((time(start)?, time(end)?)));
        match times {
            Some((start, end)) if start < DAY && end != start => Ok(Hours { start, end }),
            Some((start, end)) if start == end => {
                Err(format!("`{said}` covers no time of the day: {SAID_AS}"))
            }
            Some(_) => Err(format!("`{said}` starts at 24:00, which ends a day: {SAID_AS}")),
            None => Err(format!("`{said}` are no sleep hours: {SAID_AS}")),
        }
    }

    /// Whether a host whose local time is `local` seconds past its midnight
    /// is within them.
    pub fn hold(&self, local: u64) -> bool {
        let minute = local % DAY_SECONDS / 60;
        let (start, end) = (self.start as u64, self.end as u64);
        match start < end {
            true => start <= minute && minute < end,
            false => start <= minute || minute < end,
        }
    }

    /// How long, in seconds, a host whose local time is `local` seconds past
    /// its midnight waits for them to start: none within them.
    pub fn until(&self, local: u64) -> u64 {
        if self.hold(local) {
            return 0;
        }
        let second = local % DAY_SECONDS;
        let start = self.start as u64 * 60;
        match start > second {
            true => start - second,
            false => DAY_SECONDS - second + start,
        }
    }
}

impl std::fmt::Display for Hours {
    /// As they are said: `00:00-06:00`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (start, end) = (self.start, self.end);
        write!(f, "{:02}:{:02}-{:02}:{:02}", start / 60, start % 60, end / 60, end % 60)
    }
}

/// A time of day as said, `HH:MM` → minutes past midnight, up to 24:00.
#[spec(fn(&str) -> Option<u32{minutes: minutes <= DAY}>)]
fn time(said: &str) -> Option<u32> {
    let (hours, minutes) = said.split_once(':')?;
    let digits = |said: &str, most: usize| {
        (1..=most).contains(&said.len()) && said.bytes().all(|byte| byte.is_ascii_digit())
    };
    if !digits(hours, 2) || !digits(minutes, 2) || minutes.len() != 2 {
        return None;
    }
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    match hours < 24 && minutes < 60 {
        true => Some(hours * 60 + minutes),
        false if hours == 24 && minutes == 0 => Some(DAY),
        false => None,
    }
}

/// What a fleet instance answers when another on its host asks after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub instance: String,
    /// The repo it works, as `owner/repo` or its checkout's path.
    pub repo: String,
    /// The afkconvoy it was built from.
    pub version: String,
    /// Whether it was told `--no-host-sleep`.
    pub no_host_sleep: bool,
    /// Whether it is idle now.
    pub idle: bool,
    /// Since when, in epoch seconds, it has been idle — or not idle.
    pub since: u64,
    /// When it last worked — held a claim, or had something on its landing
    /// train — in epoch seconds; `None`: never, since it started.
    pub worked: Option<u64>,
}

/// Whether an instance with nothing held — no claim on an issue, a patrol's
/// run or a proposal — and `frontier` issues left on its frontier, `on_train`
/// saying whether anything of its is on the landing train, is idle.
pub fn idle(held: usize, frontier: usize, on_train: bool) -> bool {
    held == 0 && frontier == 0 && !on_train
}

/// Whether an instance whose last tick held `held` and had `on_train` worked
/// in it.
pub fn worked(held: usize, on_train: bool) -> bool {
    held > 0 || on_train
}

/// One instance of the host, as asking it came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Answered(Standing),
    /// It was asked, and did not answer in time.
    Silent {
        instance: String,
    },
}

/// One thing that keeps the host awake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Awake {
    /// The instance deciding has not worked since the host last woke.
    NotWorked,
    /// When the host last woke could not be read.
    WakeUnread,
    /// How long the keyboard and mouse have gone untouched could not be read.
    TouchUnread,
    /// The host's local time is outside its sleep hours, these.
    Outside {
        hours: Hours,
    },
    /// The host's local time could not be read.
    LocalUnread,
    /// The keyboard or mouse was touched this long ago, in seconds.
    Touched {
        untouched: u64,
    },
    Silent {
        instance: String,
    },
    /// It was told `--no-host-sleep`.
    Told {
        instance: String,
    },
    Busy {
        instance: String,
    },
    /// Idle, for this long only, in seconds.
    NotLongIdle {
        instance: String,
        idle_for: u64,
    },
}

impl Awake {
    /// The thing, as a line and `doctor` say it.
    pub fn said(&self) -> String {
        match self {
            Awake::NotWorked => "this instance has not worked since the host last woke".to_string(),
            Awake::WakeUnread => "when the host last woke could not be read".to_string(),
            Awake::TouchUnread => {
                "how long the keyboard and mouse have gone untouched could not be read".to_string()
            }
            Awake::Outside { hours } => {
                format!("the host's local time is outside its sleep hours, {hours}")
            }
            Awake::LocalUnread => "the host's local time could not be read".to_string(),
            Awake::Touched { untouched } => {
                format!("the keyboard or mouse was used {untouched}s ago")
            }
            Awake::Silent { instance } => format!("{instance} did not answer in time"),
            Awake::Told { instance } => format!("{instance} was told --no-host-sleep"),
            Awake::Busy { instance } => format!("{instance} is not idle"),
            Awake::NotLongIdle { instance, idle_for } => {
                format!("{instance} has been idle {idle_for}s only")
            }
        }
    }
}

/// What the host's last check found.
pub struct Host {
    /// When the host last woke, in epoch seconds: zero for a host that never
    /// slept since it booted; `None`: it could not be read.
    pub woke: Option<u64>,
    /// How long, in seconds, the keyboard and mouse have gone untouched;
    /// `None`: it could not be read.
    pub untouched: Option<u64>,
    /// Its local time, in seconds past its midnight; `None`: it could not be
    /// read.
    pub local: Option<u64>,
}

/// What keeps the host awake at `now`, for an instance that last worked at
/// `worked`, given `host`, its sleep hours `hours`, and what every instance on
/// it — itself included — answered, each having to have been idle `wait`
/// seconds and the keyboard and mouse untouched `IDLE_WAIT`: nothing, and the
/// host is put to sleep.
pub fn awake(
    worked: Option<u64>,
    host: &Host,
    hours: &Hours,
    asked: &[Asked],
    now: u64,
    wait: u64,
) -> Vec<Awake> {
    let mut awake = vec![];
    match host.local {
        None => awake.push(Awake::LocalUnread),
        Some(local) if !hours.hold(local) => awake.push(Awake::Outside { hours: *hours }),
        Some(_) => {}
    }
    match host.woke {
        None => awake.push(Awake::WakeUnread),
        Some(woke) if !worked.is_some_and(|worked| worked >= woke) => awake.push(Awake::NotWorked),
        Some(_) => {}
    }
    match host.untouched {
        None => awake.push(Awake::TouchUnread),
        Some(untouched) if untouched < IDLE_WAIT => awake.push(Awake::Touched { untouched }),
        Some(_) => {}
    }
    for one in asked {
        let standing = match one {
            Asked::Answered(standing) => standing,
            Asked::Silent { instance } => {
                awake.push(Awake::Silent { instance: instance.clone() });
                continue;
            }
        };
        let instance = standing.instance.clone();
        let idle_for = now.saturating_sub(standing.since);
        if standing.no_host_sleep {
            awake.push(Awake::Told { instance });
        } else if !standing.idle {
            awake.push(Awake::Busy { instance });
        } else if idle_for < wait {
            awake.push(Awake::NotLongIdle { instance, idle_for });
        }
    }
    awake
}

/// When an instance idle since `idle_since` (`None`: not idle) checks next,
/// having last checked at `checked`: once it has been idle `wait` seconds,
/// whatever the hour, and after a check since that kept the host awake, `wait`
/// seconds after it and within `hours`, the host's sleep hours — `local`
/// saying how far past the host's local midnight an instant is, where it can.
/// The first check outside them says so, and the next is as they start.
pub fn check_at(
    idle_since: Option<u64>,
    checked: Option<u64>,
    wait: u64,
    hours: &Hours,
    local: impl Fn(u64) -> Option<u64>,
) -> Option<u64> {
    let idle_since = idle_since?;
    let first = idle_since.saturating_add(wait);
    let Some(checked) = checked.filter(|checked| *checked >= idle_since) else {
        return Some(first);
    };
    let due = first.max(checked.saturating_add(wait));
    Some(match local(due) {
        Some(local) => due.saturating_add(hours.until(local)),
        None => due,
    })
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// Sleep hours that cover no time of the day.
    #[should_fail]
    fn hours_that_end_as_they_start() -> Hours {
        Hours { start: 360, end: 360 }
    }

    /// Sleep hours that start past the day's last minute.
    #[should_fail]
    fn hours_that_start_at_the_end_of_the_day() -> Hours {
        Hours { start: DAY, end: 360 }
    }

    /// Whatever two times were read, taken as sleep hours.
    #[should_fail]
    fn hours_of_whatever_was_read(start: u32, end: u32) -> Hours {
        Hours { start, end }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn standing(instance: &str, idle: bool, since: u64) -> Standing {
        Standing {
            instance: instance.into(),
            repo: "o/r".into(),
            version: "0.4.3".into(),
            no_host_sleep: false,
            idle,
            since,
            worked: Some(100),
        }
    }

    const HOST: Host = Host { woke: Some(50), untouched: Some(900), local: Some(3600) };

    /// Any hour.
    fn always() -> Hours {
        Hours::parse("00:00-24:00").unwrap()
    }

    /// Seconds past midnight, of `HH:MM` said as `hh` and `mm`.
    fn at(hh: u64, mm: u64) -> u64 {
        hh * 3600 + mm * 60
    }

    #[test]
    fn every_instance_long_idle_and_the_host_untouched_puts_it_to_sleep() {
        let asked = [
            Asked::Answered(standing("alpha", true, 1000)),
            Asked::Answered(standing("beta", true, 1000)),
        ];
        assert_eq!(awake(Some(100), &HOST, &SLEEP_HOURS, &asked, 1600, IDLE_WAIT), []);
        assert_eq!(
            awake(Some(100), &HOST, &SLEEP_HOURS, &[], 1600, IDLE_WAIT),
            [],
            "alone on the host"
        );
    }

    #[test]
    fn each_thing_that_keeps_the_host_awake_is_said() {
        let mut told = standing("gamma", true, 0);
        told.no_host_sleep = true;
        let asked = [
            Asked::Answered(standing("alpha", false, 1500)),
            Asked::Answered(standing("beta", true, 1200)),
            Asked::Answered(told),
            Asked::Silent { instance: "delta".into() },
        ];
        let host = Host { woke: Some(50), untouched: Some(30), local: Some(at(12, 0)) };
        let said: Vec<String> = awake(Some(100), &host, &SLEEP_HOURS, &asked, 1600, IDLE_WAIT)
            .iter()
            .map(Awake::said)
            .collect();
        assert_eq!(
            said,
            [
                "the host's local time is outside its sleep hours, 00:00-06:00",
                "the keyboard or mouse was used 30s ago",
                "alpha is not idle",
                "beta has been idle 400s only",
                "gamma was told --no-host-sleep",
                "delta did not answer in time",
            ]
        );
    }

    #[test]
    fn a_host_woken_since_the_instance_last_worked_stays_awake() {
        let woken = Host { woke: Some(200), untouched: Some(900), local: Some(0) };
        assert_eq!(
            awake(Some(100), &woken, &SLEEP_HOURS, &[], 1600, IDLE_WAIT),
            [Awake::NotWorked]
        );
        assert_eq!(
            awake(None, &HOST, &SLEEP_HOURS, &[], 1600, IDLE_WAIT),
            [Awake::NotWorked],
            "never worked"
        );
        // a host that never slept since it booted: any work counts
        let never = Host { woke: Some(0), untouched: Some(900), local: Some(0) };
        assert_eq!(awake(Some(1), &never, &SLEEP_HOURS, &[], 1600, IDLE_WAIT), []);
        let unread = Host { woke: None, untouched: None, local: None };
        assert_eq!(
            awake(Some(100), &unread, &SLEEP_HOURS, &[], 1600, IDLE_WAIT),
            [Awake::LocalUnread, Awake::WakeUnread, Awake::TouchUnread]
        );
    }

    #[test]
    fn idle_is_nothing_held_nothing_on_the_train_and_an_empty_frontier() {
        assert!(idle(0, 0, false));
        assert!(!idle(1, 0, false) && !idle(0, 2, false) && !idle(0, 0, true));
        assert!(worked(1, false) && worked(0, true) && !worked(0, false));
    }

    #[test]
    fn a_check_is_due_once_idle_the_wait_and_again_a_wait_after_the_last() {
        let utc = |at: u64| Some(at % 86_400);
        let check_at =
            |idle_since, checked| check_at(idle_since, checked, IDLE_WAIT, &always(), utc);
        assert_eq!(check_at(None, Some(10)), None, "not idle: no check");
        assert_eq!(check_at(Some(1000), None), Some(1600));
        assert_eq!(check_at(Some(1000), Some(1700)), Some(2300));
        assert_eq!(check_at(Some(1000), Some(500)), Some(1600));
        assert_eq!(check_at(Some(u64::MAX), None), Some(u64::MAX));
        assert_eq!(check_at(Some(u64::MAX - 10), Some(u64::MAX)), Some(u64::MAX));
    }

    #[test]
    fn a_check_outside_the_sleep_hours_is_followed_by_one_as_they_start() {
        // 2026-10-23 in UTC+9: the host's midnight is 15:00 UTC the day before
        let day = 1_792_713_600 - 9 * 3600;
        let tokyo = |at: u64| Some((at + 9 * 3600) % 86_400);
        let check_at =
            |idle_since, checked| check_at(idle_since, checked, IDLE_WAIT, &SLEEP_HOURS, tokyo);
        // idle since 23:00: the first check, at 23:10, says it is outside them
        let idle = day + at(23, 0);
        assert_eq!(check_at(Some(idle), None), Some(day + at(23, 10)));
        let host = Host { woke: Some(0), untouched: Some(900), local: tokyo(day + at(23, 10)) };
        let kept = awake(Some(1), &host, &SLEEP_HOURS, &[], day + at(23, 10), IDLE_WAIT);
        assert_eq!(kept, [Awake::Outside { hours: SLEEP_HOURS }]);
        // the next is as they start, at midnight, and puts the host to sleep
        let midnight = day + at(24, 0);
        assert_eq!(check_at(Some(idle), Some(day + at(23, 10))), Some(midnight));
        let host = Host { local: tokyo(midnight), ..host };
        assert_eq!(awake(Some(1), &host, &SLEEP_HOURS, &[], midnight, IDLE_WAIT), []);
        // within them, a check that kept the host awake is followed a wait after
        let one = day + at(25, 0);
        assert_eq!(check_at(Some(idle), Some(one)), Some(one + IDLE_WAIT));
        // a check at 05:55 that kept it awake: the next is at the next midnight
        let late = day + at(29, 55);
        assert_eq!(check_at(Some(idle), Some(late)), Some(day + at(48, 0)));
        // a check of an idle span before this one counts for nothing
        assert_eq!(check_at(Some(idle), Some(idle - 1)), Some(day + at(23, 10)));
        // a local time that cannot be read: a wait after, whatever the hour
        let checked = day + at(23, 10);
        let unread = super::check_at(Some(idle), Some(checked), IDLE_WAIT, &SLEEP_HOURS, |_| None);
        assert_eq!(unread, Some(day + at(23, 20)));
    }

    #[test]
    fn the_sleep_hours_hold_from_their_start_included_to_their_end_not() {
        assert!(SLEEP_HOURS.hold(at(0, 0)) && SLEEP_HOURS.hold(at(5, 59) + 59));
        assert!(!SLEEP_HOURS.hold(at(6, 0)) && !SLEEP_HOURS.hold(at(23, 59)));
        assert_eq!(SLEEP_HOURS.until(at(5, 59)), 0);
        assert_eq!(SLEEP_HOURS.until(at(6, 0)), 18 * 3600);
        assert_eq!(SLEEP_HOURS.until(at(23, 59) + 30), 30);
        // past midnight
        let night = Hours::parse("22:00-06:00").unwrap();
        assert!(night.hold(at(22, 0)) && night.hold(at(23, 59)) && night.hold(at(0, 0)));
        assert!(night.hold(at(5, 59)) && !night.hold(at(6, 0)) && !night.hold(at(21, 59)));
        assert_eq!(night.until(at(12, 0)), 10 * 3600);
        // any hour, as before
        assert!((0..86_400).step_by(60).all(|local| always().hold(local)));
        // a local time past a day is of the next
        assert!(SLEEP_HOURS.hold(86_400 + at(1, 0)));
    }

    #[test]
    fn sleep_hours_are_said_as_hh_mm_and_those_that_cover_nothing_are_refused() {
        assert_eq!(SLEEP_HOURS.to_string(), "00:00-06:00");
        for said in ["00:00-06:00", "22:00-06:00", "00:00-24:00", "23:30-00:15", "9:05-17:00"] {
            let hours = Hours::parse(said).unwrap();
            assert_eq!(Hours::parse(&hours.to_string()), Ok(hours), "{said}");
        }
        assert_eq!(Hours::parse("9:05-17:00").unwrap().to_string(), "09:05-17:00");
        assert_eq!(Hours::parse("22:00-24:00").unwrap().until(at(23, 0)), 0);
        for (said, why) in [
            ("06:00-06:00", "covers no time of the day"),
            ("24:00-24:00", "covers no time of the day"),
            ("24:00-06:00", "starts at 24:00"),
            ("00:00-24:01", "are no sleep hours"),
            ("00:00-25:00", "are no sleep hours"),
            ("00:60-06:00", "are no sleep hours"),
            ("0:0-6:00", "are no sleep hours"),
            ("+1:00-06:00", "are no sleep hours"),
            ("00:00", "are no sleep hours"),
            ("midnight-six", "are no sleep hours"),
            ("", "are no sleep hours"),
        ] {
            let error = Hours::parse(said).unwrap_err();
            assert!(error.contains(why), "{said:?}: {error}");
            assert!(error.contains("HH:MM-HH:MM"), "it names what it takes: {error}");
        }
    }

    proptest! {
        #[test]
        fn a_host_that_waits_until_its_sleep_hours_start_is_within_them(
            start in 0u32..DAY,
            end in 0u32..=DAY,
            local in any::<u64>(),
        ) {
            prop_assume!(start != end);
            let hours = Hours { start, end };
            let wait = hours.until(local);
            prop_assert!(wait < DAY_SECONDS);
            prop_assert!(hours.hold(local % DAY_SECONDS + wait));
            prop_assert_eq!(wait == 0, hours.hold(local));
            // and a minute sooner it was not, where it waited one
            if wait >= 60 {
                prop_assert!(!hours.hold((local % DAY_SECONDS) + wait - 60));
            }
        }
    }
}

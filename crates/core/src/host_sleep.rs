//! Host sleep: a fleet instance putting its host to sleep once every fleet
//! instance on it is idle (docs/adr/0016). What keeps the host awake is
//! decided here, from how each instance stands, how long the keyboard and
//! mouse have gone untouched and when the host last woke; holding how the
//! instances stand, reading the host and putting it to sleep are the effect
//! layer's.

/// How long, in seconds, every instance must have been idle — and the
/// keyboard and mouse untouched — before the host is put to sleep: ten
/// minutes. It is also how long a check that kept the host awake waits before
/// the next.
pub const IDLE_WAIT: u64 = 600;

/// How a fleet instance stands, as its host runner holds it for host sleep:
/// what it said after its last tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    pub instance: String,
    /// Whether the host settings kept the host awake as it last said.
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

/// One thing that keeps the host awake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Awake {
    /// The instance deciding has not worked since the host last woke.
    NotWorked,
    /// When the host last woke could not be read.
    WakeUnread,
    /// How long the keyboard and mouse have gone untouched could not be read.
    TouchUnread,
    /// The keyboard or mouse was touched this long ago, in seconds.
    Touched {
        untouched: u64,
    },
    /// The instance deciding was woken while it waited for the others to
    /// tick: something changed for its fleet.
    Woken,
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
            Awake::Touched { untouched } => {
                format!("the keyboard or mouse was used {untouched}s ago")
            }
            Awake::Woken => {
                "this instance was woken while it waited for the others to tick".to_string()
            }
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
}

/// What keeps the host awake at `now`, for an instance that last worked at
/// `worked`, given `host` and how every instance on it — itself included —
/// stands, each having to have been idle `wait` seconds and the keyboard and
/// mouse untouched `IDLE_WAIT`: nothing, and the host is put to sleep.
pub fn awake(
    worked: Option<u64>,
    host: &Host,
    standings: &[Standing],
    now: u64,
    wait: u64,
) -> Vec<Awake> {
    let mut awake = vec![];
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
    for standing in standings {
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
/// and `wait` seconds after a check that kept the host awake.
pub fn check_at(idle_since: Option<u64>, checked: Option<u64>, wait: u64) -> Option<u64> {
    let due = idle_since?.saturating_add(wait);
    Some(match checked {
        Some(checked) => due.max(checked.saturating_add(wait)),
        None => due,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(instance: &str, idle: bool, since: u64) -> Standing {
        Standing { instance: instance.into(), no_host_sleep: false, idle, since, worked: Some(100) }
    }

    const HOST: Host = Host { woke: Some(50), untouched: Some(900) };

    #[test]
    fn every_instance_long_idle_and_the_host_untouched_puts_it_to_sleep() {
        let standings = [standing("alpha", true, 1000), standing("beta", true, 1000)];
        assert_eq!(awake(Some(100), &HOST, &standings, 1600, IDLE_WAIT), []);
        assert_eq!(awake(Some(100), &HOST, &[], 1600, IDLE_WAIT), [], "alone on the host");
    }

    #[test]
    fn each_thing_that_keeps_the_host_awake_is_said() {
        let mut told = standing("gamma", true, 0);
        told.no_host_sleep = true;
        let standings = [standing("alpha", false, 1500), standing("beta", true, 1200), told];
        let host = Host { woke: Some(50), untouched: Some(30) };
        let said: Vec<String> =
            awake(Some(100), &host, &standings, 1600, IDLE_WAIT).iter().map(Awake::said).collect();
        assert_eq!(
            said,
            [
                "the keyboard or mouse was used 30s ago",
                "alpha is not idle",
                "beta has been idle 400s only",
                "gamma was told --no-host-sleep",
            ]
        );
        assert_eq!(
            Awake::Woken.said(),
            "this instance was woken while it waited for the others to tick"
        );
    }

    #[test]
    fn a_host_woken_since_the_instance_last_worked_stays_awake() {
        let woken = Host { woke: Some(200), untouched: Some(900) };
        assert_eq!(awake(Some(100), &woken, &[], 1600, IDLE_WAIT), [Awake::NotWorked]);
        assert_eq!(awake(None, &HOST, &[], 1600, IDLE_WAIT), [Awake::NotWorked], "never worked");
        // a host that never slept since it booted: any work counts
        let never = Host { woke: Some(0), untouched: Some(900) };
        assert_eq!(awake(Some(1), &never, &[], 1600, IDLE_WAIT), []);
        let unread = Host { woke: None, untouched: None };
        assert_eq!(
            awake(Some(100), &unread, &[], 1600, IDLE_WAIT),
            [Awake::WakeUnread, Awake::TouchUnread]
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
        assert_eq!(check_at(None, Some(10), IDLE_WAIT), None, "not idle: no check");
        assert_eq!(check_at(Some(1000), None, IDLE_WAIT), Some(1600));
        assert_eq!(check_at(Some(1000), Some(1700), IDLE_WAIT), Some(2300));
        assert_eq!(check_at(Some(1000), Some(500), IDLE_WAIT), Some(1600));
        assert_eq!(check_at(Some(u64::MAX), None, IDLE_WAIT), Some(u64::MAX));
    }
}

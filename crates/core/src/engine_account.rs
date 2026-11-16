//! Which engine account a worker starts on, of the several of its harness
//! (GLOSSARY: **Account**; sunfmin/control#1220): the one with the most
//! left of those that take work. Whether one takes work is decided here,
//! from what the effect layer read of it — its usage windows, as its
//! engine last reported them, a limit it met, how many workers run on it
//! and whether its session can start on this machine; reading them is the
//! effect layer's.

use serde::{Deserialize, Serialize};

/// The share of a usage window, in percent, at or past which an account
/// takes no new worker until that window resets, where the host says none
/// (`--account-limit`). The engine reports the whole account — its owner's
/// own use among it — so its figure stands as it is.
pub const LIMIT: u64 = 90;

/// How long, in seconds, an account that met a limit rests where the limit
/// names no reset: an hour.
pub const REST: u64 = 3600;

/// One usage window of an account, as its engine last reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// The engine's own name for it: `five_hour`, `seven_day`, `primary`,
    /// `secondary`, `limit_5h`.
    pub name: String,
    /// How much of it is used, in percent.
    pub used: u64,
    /// When it resets, in unix seconds.
    pub resets_at: u64,
}

/// Whether an account's session can start on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum Here {
    /// It can start here.
    Ready,
    /// Its login is bound to another machine.
    Elsewhere { machine: String },
    /// Its login is to be made here first, by a person running `line`.
    LoggedOut { line: String },
}

/// What the effect layer read of one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Standing {
    /// Its name.
    pub name: String,
    /// Its harness, by its key.
    pub harness: String,
    /// Its windows, as its engine last reported them; none: its engine
    /// reports none, or has not yet.
    pub windows: Vec<Window>,
    /// Until when, in unix seconds, it rests since it met a limit; `None`:
    /// it met none that still holds.
    pub rests_until: Option<u64>,
    /// How many workers run on it.
    pub running: usize,
    /// How many may run on it at once; `None`: it says no bound.
    pub sessions: Option<u32>,
    pub here: Here,
}

/// Why an account takes no new worker now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Refused {
    /// Its window `window` is `used` percent used, at or past `limit`, until
    /// it resets at `resets_at`.
    Window { window: String, used: u64, limit: u64, resets_at: u64 },
    /// It met a limit, and rests until `until`.
    Resting { until: u64 },
    /// `running` workers run on it, as many as it takes at once.
    Full { running: usize, sessions: u32 },
    /// Its login is bound to `machine`.
    Elsewhere { machine: String },
    /// Its login is to be made here first.
    LoggedOut { line: String },
}

impl Refused {
    /// How a tick's line, `plan` and the panel say it.
    pub fn said(&self, now: u64) -> String {
        match self {
            Refused::Window { window, used, limit, resets_at } => format!(
                "its `{window}` window is {used}% used, at or past {limit}%, and resets in {}",
                minutes(resets_at.saturating_sub(now))
            ),
            Refused::Resting { until } => {
                format!("it met a limit and rests for {} more", minutes(until.saturating_sub(now)))
            }
            Refused::Full { running, sessions } => {
                format!("{running} worker(s) run on it, as many as its {sessions} session(s)")
            }
            Refused::Elsewhere { machine } => format!("its login is on {machine}"),
            Refused::LoggedOut { line } => format!("its login is to be made here: {line}"),
        }
    }
}

/// `seconds` in whole minutes, as a span is said: `14 min`.
fn minutes(seconds: u64) -> String {
    format!("{} min", seconds.div_ceil(60))
}

impl Standing {
    /// The windows that still hold `now`: a window that has reset since its
    /// engine reported it says nothing of it any more.
    pub fn current(&self, now: u64) -> impl Iterator<Item = &Window> {
        self.windows.iter().filter(move |window| window.resets_at > now)
    }

    /// Why it takes no new worker `now`, its windows judged against `limit`
    /// percent; `None`: it takes one.
    pub fn refused(&self, limit: u64, now: u64) -> Option<Refused> {
        match &self.here {
            Here::Ready => {}
            Here::Elsewhere { machine } => {
                return Some(Refused::Elsewhere { machine: machine.clone() });
            }
            Here::LoggedOut { line } => return Some(Refused::LoggedOut { line: line.clone() }),
        }
        if let Some(until) = self.rests_until.filter(|until| *until > now) {
            return Some(Refused::Resting { until });
        }
        if let Some(window) = self
            .current(now)
            .filter(|window| window.used >= limit)
            .max_by_key(|window| (window.used, window.resets_at))
        {
            return Some(Refused::Window {
                window: window.name.clone(),
                used: window.used,
                limit,
                resets_at: window.resets_at,
            });
        }
        if let Some(sessions) = self.sessions.filter(|sessions| self.running >= *sessions as usize)
        {
            return Some(Refused::Full { running: self.running, sessions });
        }
        None
    }

    /// How much is left of it `now`, in percent: what is left of its most
    /// used window that still holds. An account whose engine has reported
    /// no window that holds has all of it left.
    pub fn left(&self, now: u64) -> u64 {
        let used = self.current(now).map(|window| window.used).max().unwrap_or(0);
        100u64.saturating_sub(used)
    }
}

/// The account of `standings` a new worker starts on `now`: of those that
/// take one (`Standing::refused`, against `limit`), the one with the most
/// left; of equals, the one with the fewest workers on it, then the first.
/// `None`: none takes one, and its harness's workers wait.
pub fn choose(standings: &[Standing], limit: u64, now: u64) -> Option<&Standing> {
    standings
        .iter()
        .enumerate()
        .filter(|(_, standing)| standing.refused(limit, now).is_none())
        .min_by_key(|(at, standing)| (std::cmp::Reverse(standing.left(now)), standing.running, *at))
        .map(|(_, standing)| standing)
}

/// Until when an account that met a limit at `at` rests: the reset the
/// limit names, where it names one after `at`, else `REST` after it.
pub fn rest_until(at: u64, resets_at: Option<u64>) -> u64 {
    resets_at.filter(|reset| *reset > at).unwrap_or(at.saturating_add(REST))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn claude(name: &str, five_hour: u64) -> Standing {
        Standing {
            name: name.into(),
            harness: "claude-code".into(),
            windows: vec![Window {
                name: "five_hour".into(),
                used: five_hour,
                resets_at: NOW + 3600,
            }],
            rests_until: None,
            running: 0,
            sessions: None,
            here: Here::Ready,
        }
    }

    #[test]
    fn a_worker_starts_on_the_account_with_most_left() {
        let standings = [claude("a", 92), claude("b", 40), claude("c", 10)];
        assert_eq!(choose(&standings, LIMIT, NOW).map(|s| s.name.as_str()), Some("c"));
    }

    #[test]
    fn an_account_at_ninety_percent_of_a_window_takes_no_new_worker() {
        let standings = [claude("a", 92), claude("b", 60)];
        assert_eq!(choose(&standings, LIMIT, NOW).map(|s| s.name.as_str()), Some("b"));
        assert_eq!(
            claude("a", 90).refused(LIMIT, NOW),
            Some(Refused::Window {
                window: "five_hour".into(),
                used: 90,
                limit: 90,
                resets_at: NOW + 3600
            })
        );
        assert_eq!(claude("a", 89).refused(LIMIT, NOW), None);
        assert_eq!(claude("a", 92).refused(95, NOW), None, "the limit is the host's");
    }

    #[test]
    fn with_every_account_past_its_limit_none_starts_until_a_window_resets() {
        let standings = [claude("a", 92), claude("b", 97)];
        assert_eq!(choose(&standings, LIMIT, NOW), None);
        let later = NOW + 3601;
        assert_eq!(choose(&standings, LIMIT, later).map(|s| s.name.as_str()), Some("a"));
    }

    #[test]
    fn an_account_that_met_a_limit_rests_until_its_reset_or_an_hour() {
        let mut grok = claude("g", 0);
        grok.harness = "grok".into();
        grok.windows.clear();
        grok.rests_until = Some(rest_until(NOW, Some(NOW + 600)));
        assert_eq!(grok.refused(LIMIT, NOW), Some(Refused::Resting { until: NOW + 600 }));
        assert_eq!(choose(std::slice::from_ref(&grok), LIMIT, NOW), None);
        assert_eq!(grok.refused(LIMIT, NOW + 600), None, "it takes work again at the reset");
        assert_eq!(rest_until(NOW, None), NOW + REST);
        assert_eq!(rest_until(NOW, Some(NOW - 5)), NOW + REST, "a reset already past names none");
    }

    #[test]
    fn an_account_takes_no_more_workers_than_its_sessions() {
        let mut a = claude("a", 10);
        a.sessions = Some(2);
        a.running = 2;
        let b = claude("b", 50);
        assert_eq!(a.refused(LIMIT, NOW), Some(Refused::Full { running: 2, sessions: 2 }));
        assert_eq!(choose(&[a, b], LIMIT, NOW).map(|s| s.name.as_str()), Some("b"));
    }

    #[test]
    fn an_account_whose_session_cannot_start_here_takes_none() {
        let mut a = claude("a", 0);
        a.here = Here::Elsewhere { machine: "mini".into() };
        let mut b = claude("b", 0);
        b.here = Here::LoggedOut { line: "codex login".into() };
        assert_eq!(choose(&[a, b], LIMIT, NOW), None);
    }

    #[test]
    fn of_equals_the_one_with_fewest_workers_then_the_first() {
        let mut a = claude("a", 30);
        a.running = 1;
        let b = claude("b", 30);
        let c = claude("c", 30);
        assert_eq!(choose(&[a, b, c], LIMIT, NOW).map(|s| s.name.as_str()), Some("b"));
    }
}

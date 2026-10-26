//! Host sleep's effect half (docs/adr/0016): how every fleet instance of the
//! host runner stands, held by the host runner itself (docs/adr/0023); the
//! reading of the host — when it last woke, how long its keyboard and mouse
//! have gone untouched, its local time — and `pmset sleepnow`. What keeps the host awake is
//! the core's to decide (`afkconvoy_core::host_sleep::awake`).
//!
//! Each fleet instance says how it stands as each of its ticks ends
//! (`HostSleep::ticked`), and a check reads how every one stands from there:
//! none is asked. One check is made on the host at a time. Before it puts the
//! host to sleep, it wakes every other instance to tick once more, ticks once
//! more itself, and waits until each has ended a tick it began after it was
//! woken — or has ended — to find each still idle.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use afkconvoy_core::host_runner::Stands;
use afkconvoy_core::host_sleep::{Awake, Host, Hours, SLEEP_HOURS, Standing, awake, check_at};
use anyhow::{Context, Result, bail};

use crate::host_settings;
use crate::stop::Stop;
use crate::wake::{Waker, Wakes};

/// How often a check waiting for the others' ticks looks whether its `run`
/// was told to stop, or woken.
const LOOKS_EVERY: Duration = Duration::from_millis(250);

/// How every fleet instance of one host runner stands, as host sleep reads
/// it: each from its `HostSleep::open` until that is dropped, and which of
/// them makes the check underway. A clone is the same.
#[derive(Clone, Default)]
pub struct Standings {
    held: Arc<(Mutex<Held>, Condvar)>,
}

#[derive(Default)]
struct Held {
    /// Each fleet instance, by the key it opened with.
    each: BTreeMap<u64, One>,
    /// The key the next instance to open takes.
    next: u64,
    /// The instance whose check is underway: one at a time.
    checking: Option<u64>,
    /// No new tick begins while the host runner prepares a release move.
    paused: bool,
}

/// One fleet instance, as it last said it stands.
struct One {
    standing: Standing,
    /// How many ticks it began, and how many it ended.
    began: u64,
    ended: u64,
    /// Whether its last tick held a claim, had something on the landing
    /// train, or failed whole: so until its first has ended.
    at_work: bool,
    /// How it is woken to tick.
    waker: Waker,
}

impl Standings {
    pub fn pause(&self) {
        self.held().paused = true;
    }

    pub fn resume(&self) {
        self.held().paused = false;
        self.held.1.notify_all();
    }
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How every instance stands now.
    fn every(held: &Held) -> Vec<Standing> {
        held.each.values().map(|one| one.standing.clone()).collect()
    }

    /// Changes the instance `key` as `change` says, and tells a check that
    /// waits for it.
    fn change(&self, key: u64, change: impl FnOnce(&mut One)) {
        if let Some(one) = self.held().each.get_mut(&key) {
            change(one);
        }
        self.held.1.notify_all();
    }

    /// Wakes every instance but `key` to tick → each, by its key, with the
    /// ticks it had begun as it was woken.
    fn wake_others(&self, key: u64) -> Vec<(u64, u64)> {
        let others: Vec<(u64, u64, Waker)> = self
            .held()
            .each
            .iter()
            .filter(|(other, _)| **other != key)
            .map(|(other, one)| (*other, one.began, one.waker.clone()))
            .collect();
        // Outside the lock: a wake may be said in the woken one's log.
        let woken = |(other, began, waker): (u64, u64, Waker)| {
            waker.wake();
            (other, began)
        };
        others.into_iter().map(woken).collect()
    }

    /// Waits until each of `asked` — an instance by its key, with the ticks
    /// it had begun as it was woken — has ended a tick it began since, or has
    /// ended, and no longer once `given_up` → how every instance stands then;
    /// `None` where it gave up first.
    fn ticked_since(
        &self,
        asked: &[(u64, u64)],
        given_up: impl Fn() -> bool,
    ) -> Option<Vec<Standing>> {
        let mut held = self.held();
        loop {
            let waits = |(key, began): &(u64, u64)| {
                held.each.get(key).is_some_and(|one| one.ended <= *began)
            };
            if !asked.iter().any(waits) {
                return Some(Standings::every(&held));
            }
            if given_up() {
                return None;
            }
            let waited = self.held.1.wait_timeout(held, LOOKS_EVERY);
            held = waited.map_or_else(|poisoned| poisoned.into_inner().0, |(held, _)| held);
        }
    }

    /// Calls `with` with how every instance stands now, as a move of their
    /// host runner reads it — each by its id: `Ticking` from the start of a
    /// tick until it ends, else `AtWork` or `Quiet` as its last tick left it —
    /// and no instance begins a tick until `with` returns.
    pub fn held_still<T>(&self, with: impl FnOnce(&BTreeMap<String, Stands>) -> T) -> T {
        let held = self.held();
        let stands = |one: &One| match one {
            One { began: 0, .. } => Stands::Starting,
            One { began, ended, .. } if began > ended => Stands::Ticking,
            One { at_work: true, .. } => Stands::AtWork,
            One { at_work: false, .. } => Stands::Quiet,
        };
        let each = held.each.values().map(|one| (one.standing.instance.clone(), stands(one)));
        with(&each.collect())
    }

    /// Lets the check of the instance `key` go, if it makes it: another may
    /// check from now on.
    fn let_go(&self, key: u64) {
        let mut held = self.held();
        if held.checking == Some(key) {
            held.checking = None;
        }
    }
}

/// One check, as far as it got.
pub enum Checked {
    /// Nothing keeps the host awake so far: every other instance is woken to
    /// tick once more, and this one ticks too — the next tick is now, and the
    /// check ends after it.
    TickNow,
    /// The host stays awake, for these.
    Awake(Vec<Awake>),
    /// The host was put to sleep.
    Slept,
    /// Nothing kept the host awake, and `pmset sleepnow` failed.
    Failed(String),
    /// The `run` was told to stop while the check waited: it ended there,
    /// with the host left awake.
    Stopped,
}

/// A fleet instance's part in host sleep: it stands among the others from
/// `open` until it is dropped, which takes it from among them.
pub struct HostSleep {
    standings: Standings,
    key: u64,
    /// Its own wakes: one heard while a check waits ends the check.
    wakes: Wakes,
    /// How long, in seconds, every instance must have been idle.
    wait: u64,
    /// When this instance last checked.
    checked: Option<u64>,
    /// The other instances woken to tick once more, each by its key with the
    /// ticks it had begun then, while a check waits for this one's own tick.
    asked: Option<Vec<(u64, u64)>>,
}

impl HostSleep {
    /// Stands among `standings` as `standing` says this instance is, woken by
    /// `wakes` when a check has it tick once more, and puts the host to sleep
    /// within its sleep hours unless the host settings keep it awake, as they
    /// say at each check. Called before the first tick.
    pub fn open(standing: Standing, wakes: &Wakes, standings: &Standings, wait: u64) -> HostSleep {
        let key = {
            let mut held = standings.held();
            let key = held.next;
            held.next += 1;
            let one = One { standing, began: 0, ended: 0, at_work: true, waker: wakes.waker() };
            held.each.insert(key, one);
            key
        };
        HostSleep {
            standings: standings.clone(),
            key,
            wakes: wakes.clone(),
            wait,
            checked: None,
            asked: None,
        }
    }

    /// Says that this instance begins a tick, without a release move's pause.
    #[cfg(test)]
    pub fn begins(&self) {
        self.standings.change(self.key, |one| one.began += 1);
    }

    /// Begins a tick only while the host runner permits it. A release move
    /// pauses here, then stops the instance without entering another tick.
    pub fn begins_unless_stopped(&self, stop: &Stop) -> bool {
        let mut held = self.standings.held();
        // A checkout is put on the list by its first tick. Let that finish
        // even when a move is pending, so the replacement finds it there.
        while held.paused
            && held.each.get(&self.key).is_some_and(|one| one.began > 0)
            && !stop.told()
        {
            held = self
                .standings
                .held
                .1
                .wait_timeout(held, LOOKS_EVERY)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        if stop.told() {
            return false;
        }
        if let Some(one) = held.each.get_mut(&self.key) {
            one.began += 1;
        }
        true
    }

    /// Says how this instance stands after a tick at `now`: idle or not, and
    /// whether it worked — `None` of a tick that failed whole, which is not
    /// idle. A check that waits for its tick is told.
    pub fn ticked(&self, now: u64, idle: Option<(bool, bool)>) {
        let kept_awake = host_settings::kept_awake();
        let at_work = idle.is_none_or(|(_, worked)| worked);
        let (idle, worked) = idle.unwrap_or((false, false));
        self.standings.change(self.key, |one| {
            one.at_work = at_work;
            let standing = &mut one.standing;
            standing.no_host_sleep = kept_awake;
            if standing.idle != idle {
                standing.since = now;
            }
            standing.idle = idle;
            if worked {
                standing.worked = Some(now);
            }
            one.ended += 1;
        });
    }

    /// How this instance stands now, with what the host settings say of
    /// host sleep read again — whether they keep the host awake is how it
    /// stands from then on — and the host's sleep hours.
    fn standing(&self) -> Option<(Standing, Hours)> {
        let host_sleep = host_settings::host_sleep();
        let mut held = self.standings.held();
        let one = held.each.get_mut(&self.key)?;
        one.standing.no_host_sleep = host_sleep.is_none();
        Some((one.standing.clone(), host_sleep.unwrap_or(SLEEP_HOURS)))
    }

    /// When this instance checks next; `None`: it does not, not being idle,
    /// the host settings keeping the host awake, or another instance's check
    /// being underway. A check of its own underway is due now.
    pub fn due(&self) -> Option<u64> {
        if self.asked.is_some() {
            return Some(0);
        }
        let (standing, hours) = self.standing()?;
        if standing.no_host_sleep || self.standings.held().checking.is_some() {
            return None;
        }
        let idle_since = standing.idle.then_some(standing.since);
        check_at(idle_since, self.checked, self.wait, &hours, local)
    }

    /// Checks at `now`, if a check is due → how far it got. A check reads how
    /// every instance stands; if nothing keeps the host awake, it has each —
    /// this one too — tick once more (`Checked::TickNow`), and ends with the
    /// call after this instance's tick, by putting the host to sleep or not.
    /// A `run` told to stop (`stop`) while a check waits is not held up by
    /// it, and does not put the host to sleep.
    pub fn check(&mut self, now: u64, clock: impl Fn() -> u64, stop: &Stop) -> Option<Checked> {
        if let Some(asked) = self.asked.take() {
            let checked = self.end(&asked, clock, stop);
            self.standings.let_go(self.key);
            return Some(checked);
        }
        if self.due().is_none_or(|due| due > now) {
            return None;
        }
        let hours = self.standing().map_or(SLEEP_HOURS, |(_, hours)| hours);
        let (worked, every) = {
            let mut held = self.standings.held();
            if held.checking.is_some() {
                return None;
            }
            held.checking = Some(self.key);
            let worked = held.each.get(&self.key).and_then(|one| one.standing.worked);
            (worked, Standings::every(&held))
        };
        self.checked = Some(now);
        let awake = awake(worked, &read_host(now), &hours, &every, now, self.wait);
        if !awake.is_empty() {
            self.standings.let_go(self.key);
            return Some(Checked::Awake(awake));
        }
        self.asked = Some(self.standings.wake_others(self.key));
        Some(Checked::TickNow)
    }

    /// Ends a check after this instance ticked: waits until every other
    /// instance it woke, `asked`, ticked once more or ended — no longer once
    /// this one is told to stop or woken — and reads how each stands then.
    fn end(&mut self, asked: &[(u64, u64)], clock: impl Fn() -> u64, stop: &Stop) -> Checked {
        let every = self.standings.ticked_since(asked, || stop.told() || self.wakes.heard());
        let now = clock();
        self.checked = Some(now);
        if stop.told() {
            return Checked::Stopped;
        }
        let Some(every) = every else { return Checked::Awake(vec![Awake::Woken]) };
        let Some((standing, hours)) = self.standing() else { return Checked::Stopped };
        let awake = awake(standing.worked, &read_host(now), &hours, &every, now, self.wait);
        if !awake.is_empty() {
            return Checked::Awake(awake);
        }
        match sleep_now() {
            Ok(()) => Checked::Slept,
            Err(error) => Checked::Failed(format!("{error:#}")),
        }
    }
}

impl Drop for HostSleep {
    fn drop(&mut self) {
        let mut held = self.standings.held();
        if held.checking == Some(self.key) {
            held.checking = None;
        }
        held.each.remove(&self.key);
        drop(held);
        // A check that waits for it waits no more.
        self.standings.held.1.notify_all();
    }
}

/// What a check at `now` reads of the host.
fn read_host(now: u64) -> Host {
    Host { woke: woke().ok(), untouched: untouched().ok(), local: local(now) }
}

/// How far past the host's local midnight the instant `at`, in epoch
/// seconds, is, in seconds, as its time zone has it (`localtime_r`, which
/// reads TZ, else /etc/localtime); `None` where that cannot be said.
#[cfg(target_os = "macos")]
fn local(at: u64) -> Option<u64> {
    let at = libc::time_t::try_from(at).ok()?;
    let mut time = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `time` is a `tm`, which `localtime_r` writes whole where it
    // returns it, and leaves alone where it returns null.
    let said = unsafe { libc::localtime_r(&at, time.as_mut_ptr()) };
    if said.is_null() {
        return None;
    }
    // SAFETY: zeroed, then written by `localtime_r`: every field is a number,
    // or a pointer nothing here reads.
    let time = unsafe { time.assume_init() };
    let (hour, minute, second) = (time.tm_hour, time.tm_min, time.tm_sec);
    u64::try_from(hour * 3600 + minute * 60 + second).ok()
}

/// Only a macOS host is put to sleep (docs/adr/0016): elsewhere its local
/// time is never asked.
#[cfg(not(target_os = "macos"))]
fn local(_at: u64) -> Option<u64> {
    None
}

/// The output of `program` with `args`, which must succeed.
pub(crate) fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("`{program}` could not be run"))?;
    if !out.status.success() {
        bail!("`{program} {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// When the host last woke, in epoch seconds — zero for one that never slept
/// since it booted — as `sysctl kern.waketime` says: `{ sec = 1760000000,
/// usec = 5 } Sat Oct 18 …`.
fn woke() -> Result<u64> {
    let said = output("sysctl", &["-n", "kern.waketime"])?;
    waketime(&said).with_context(|| format!("`sysctl kern.waketime` said {said:?}"))
}

fn waketime(said: &str) -> Option<u64> {
    let after = said.split_once("sec =")?.1;
    after.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// How long, in seconds, the keyboard and mouse have gone untouched, as
/// `ioreg` says HIDIdleTime, in nanoseconds: `"HIDIdleTime" = 1234567890`.
fn untouched() -> Result<u64> {
    let said = output("ioreg", &["-c", "IOHIDSystem", "-d", "4"])?;
    hid_idle(&said).context("`ioreg -c IOHIDSystem` says no HIDIdleTime")
}

fn hid_idle(said: &str) -> Option<u64> {
    let line = said.lines().find(|line| line.contains("\"HIDIdleTime\""))?;
    let nanos: u64 = line.split_once('=')?.1.trim().parse().ok()?;
    Some(nanos / 1_000_000_000)
}

/// Puts the host to sleep: it goes to sleep as `pmset` returns.
fn sleep_now() -> Result<()> {
    output("pmset", &["sleepnow"]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(instance: &str) -> Standing {
        Standing {
            instance: instance.into(),
            no_host_sleep: false,
            idle: true,
            since: 0,
            worked: None,
        }
    }

    fn instances(every: &[Standing]) -> Vec<&str> {
        every.iter().map(|standing| standing.instance.as_str()).collect()
    }

    #[test]
    fn how_each_instance_stands_is_held_for_every_other_and_one_drops_out_as_it_ends() {
        let standings = Standings::default();
        let (alpha_wakes, beta_wakes) = (Wakes::default(), Wakes::default());
        let alpha = HostSleep::open(standing("alpha"), &alpha_wakes, &standings, 0);
        let beta = HostSleep::open(standing("beta"), &beta_wakes, &standings, 0);
        let other = Standings::default();
        let _gamma = HostSleep::open(standing("gamma"), &Wakes::default(), &other, 0);
        let every = || Standings::every(&standings.held());
        assert_eq!(instances(&every()), ["alpha", "beta"], "another runner's are not among them");

        beta.ticked(100, Some((false, true)));
        let beta_now = every()[1].clone();
        assert_eq!((beta_now.idle, beta_now.since, beta_now.worked), (false, 100, Some(100)));
        drop(beta);
        assert_eq!(instances(&every()), ["alpha"]);
        drop(alpha);
        assert!(every().is_empty());
        assert!(beta_wakes.rest(Duration::ZERO).is_none(), "nobody was woken");
    }

    #[test]
    fn a_check_wakes_the_others_and_waits_until_each_ended_a_tick_begun_after_or_ended() {
        let standings = Standings::default();
        let (beta_wakes, gamma_wakes) = (Wakes::default(), Wakes::default());
        let alpha = HostSleep::open(standing("alpha"), &Wakes::default(), &standings, 0);
        let beta = HostSleep::open(standing("beta"), &beta_wakes, &standings, 0);
        let gamma = HostSleep::open(standing("gamma"), &gamma_wakes, &standings, 0);
        // beta is in the middle of a tick as it is woken: that tick is not the one
        beta.begins();
        let asked = standings.wake_others(alpha.key);
        assert_eq!(asked, [(beta.key, 1), (gamma.key, 0)]);
        assert!(
            beta_wakes.rest(Duration::ZERO).is_some() && gamma_wakes.rest(Duration::ZERO).is_some()
        );
        let waited = {
            let standings = standings.clone();
            let asked = asked.clone();
            std::thread::spawn(move || standings.ticked_since(&asked, || false))
        };
        beta.ticked(10, Some((true, false)));
        gamma.begins();
        gamma.ticked(11, Some((true, false)));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waited.is_finished(), "beta's tick began before it was woken");
        beta.begins();
        beta.ticked(12, Some((false, false)));
        let every = waited.join().unwrap().expect("each ticked");
        assert!(!every[1].idle, "as beta stands after its tick: {every:?}");

        // one that ends is waited for no more, and one given up waits no longer
        let asked = standings.wake_others(alpha.key);
        let waited = {
            let standings = standings.clone();
            std::thread::spawn(move || standings.ticked_since(&asked, || false))
        };
        drop(beta);
        gamma.begins();
        gamma.ticked(13, Some((true, false)));
        assert_eq!(instances(&waited.join().unwrap().unwrap()), ["alpha", "gamma"]);
        let asked = standings.wake_others(alpha.key);
        assert_eq!(standings.ticked_since(&asked, || true), None);
    }

    #[test]
    fn one_check_is_underway_on_the_host_at_a_time() {
        let standings = Standings::default();
        let alpha = HostSleep::open(standing("alpha"), &Wakes::default(), &standings, 0);
        let beta = HostSleep::open(standing("beta"), &Wakes::default(), &standings, 0);
        alpha.ticked(0, Some((true, false)));
        beta.ticked(0, Some((true, false)));
        assert_eq!(beta.due(), Some(0));
        standings.held().checking = Some(alpha.key);
        assert_eq!(beta.due(), None, "alpha's check is underway");
        standings.let_go(beta.key);
        assert_eq!(beta.due(), None, "only alpha lets its own check go");
        drop(alpha);
        assert_eq!(beta.due(), Some(0), "nor holds it once it ended");
    }

    #[test]
    fn the_host_is_read_as_sysctl_and_ioreg_say_it() {
        let said = "{ sec = 1760000000, usec = 518000 } Sat Oct 18 10:00:00 2026\n";
        assert_eq!(waketime(said), Some(1_760_000_000));
        assert_eq!(waketime("{ sec = 0, usec = 0 } Thu Jan  1 09:00:00 1970\n"), Some(0));
        assert_eq!(waketime("nothing"), None);
        let said = "    | |   \"HIDIdleTime\" = 754000000000\n    | |   \"Other\" = 1\n";
        assert_eq!(hid_idle(said), Some(754));
        assert_eq!(hid_idle("no such key"), None);
    }
}

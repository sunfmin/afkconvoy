//! Host sleep's effect half (docs/adr/0016): the asking of every other fleet
//! instance of the host runner how it stands, the reading of the host — when
//! it last woke, how long its keyboard and mouse have gone untouched — and
//! `pmset sleepnow`. What keeps the host awake is the core's to decide
//! (`afkconvoy_core::host_sleep::awake`).
//!
//! Every fleet instance of a host is held by its one host runner
//! (docs/adr/0023), so the instances it asks are those of its own process
//! (`Peers`): each is asked how it stands, or to tick first and then say
//! so, and answers with its `Standing`.

use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use afkconvoy_core::host_sleep::{
    ANSWERS_WITHIN, Asked, Awake, Host, Standing, TICKS_WITHIN, awake, check_at,
};
use anyhow::{Context, Result, bail};

use crate::host_settings;
use crate::stop::Stop;
use crate::wake::Waker;

/// What an instance answers, and the askers waiting for its next tick.
struct Shared {
    standing: Standing,
    waiting: Vec<Sender<Standing>>,
}

/// The fleet instances of one host runner, as host sleep asks them: each from
/// its `HostSleep::open` until that is dropped. A clone is the same.
#[derive(Clone, Default)]
pub struct Peers {
    open: Arc<Mutex<Vec<Peer>>>,
}

/// One fleet instance among its peers: how it stands, and how it is woken to
/// tick.
#[derive(Clone)]
struct Peer {
    shared: Arc<Mutex<Shared>>,
    waker: Waker,
}

impl Peers {
    fn open(&self) -> MutexGuard<'_, Vec<Peer>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What to ask a peer.
#[derive(Clone, Copy)]
enum Question {
    /// How it stands.
    Standing,
    /// To tick first, and then say how it stands.
    Tick,
}

/// One check, as far as it got.
pub enum Checked {
    /// Nothing keeps the host awake so far: every instance is asked to tick
    /// first, this one too — the next tick is now, and the check ends after it.
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

/// A fleet instance's part in host sleep: its peers ask it from `open` until
/// it is dropped, which takes it from among them.
pub struct HostSleep {
    peers: Peers,
    shared: Arc<Mutex<Shared>>,
    /// How long, in seconds, every instance must have been idle.
    wait: u64,
    /// When this instance last checked.
    checked: Option<u64>,
    /// The other instances asked to tick first, while a check waits for this
    /// one's own tick.
    asking: Option<Vec<Asking>>,
}

/// One instance asked, its answer to come.
struct Asking {
    instance: String,
    answer: JoinHandle<Option<Standing>>,
}

impl HostSleep {
    /// Joins `peers` as `standing` says this instance is, woken by `waker`
    /// when one asks it to tick, and puts the host to sleep unless the host
    /// settings keep it awake, as they say at each check. Called before the
    /// first tick.
    pub fn open(standing: Standing, waker: Waker, peers: &Peers, wait: u64) -> HostSleep {
        let shared = Arc::new(Mutex::new(Shared { standing, waiting: vec![] }));
        peers.open().push(Peer { shared: Arc::clone(&shared), waker });
        HostSleep { peers: peers.clone(), shared, wait, checked: None, asking: None }
    }

    /// Says how this instance stands after a tick at `now`: idle or not, and
    /// whether it worked — `None` of a tick that failed whole, which is not
    /// idle. Every asker waiting for a tick is answered.
    pub fn ticked(&self, now: u64, idle: Option<(bool, bool)>) {
        let kept_awake = host_settings::kept_awake();
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (idle, worked) = idle.unwrap_or((false, false));
        let standing = &mut shared.standing;
        standing.no_host_sleep = kept_awake;
        if standing.idle != idle {
            standing.since = now;
        }
        standing.idle = idle;
        if worked {
            standing.worked = Some(now);
        }
        let standing = standing.clone();
        for asker in std::mem::take(&mut shared.waiting) {
            let _ = asker.send(standing.clone());
        }
    }

    /// How this instance stands now, with whether the host settings keep
    /// the host awake read again: what it is answered as from then on.
    fn standing(&self) -> Standing {
        let kept_awake = host_settings::kept_awake();
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.standing.no_host_sleep = kept_awake;
        shared.standing.clone()
    }

    /// When this instance checks next; `None`: it does not, not being idle or
    /// the host settings keeping the host awake. A check underway is due now.
    pub fn due(&self) -> Option<u64> {
        let standing = self.standing();
        match self.asking {
            Some(_) => Some(0),
            None if standing.no_host_sleep => None,
            None => check_at(standing.idle.then_some(standing.since), self.checked, self.wait),
        }
    }

    /// Checks at `now`, if a check is due → how far it got. A check asks
    /// every instance how it stands; if nothing keeps the host awake, it has
    /// each — this one too — tick first (`Checked::TickNow`), and ends with the
    /// call after this instance's tick, by putting the host to sleep or not.
    /// A `run` told to stop (`stop`) while a check waits is not held up by it,
    /// and does not put the host to sleep.
    pub fn check(&mut self, now: u64, clock: impl Fn() -> u64, stop: &Stop) -> Option<Checked> {
        if let Some(asking) = self.asking.take() {
            return Some(self.end(asking, clock, stop));
        }
        if self.due().is_none_or(|due| due > now) {
            return None;
        }
        self.checked = Some(now);
        let standing = self.standing();
        let mut asked = vec![Asked::Answered(standing.clone())];
        let answers = Duration::from_secs(ANSWERS_WITHIN);
        asked.extend(wait_for(self.ask(Question::Standing), answers, || {}, || stop.told()));
        if stop.told() {
            return Some(Checked::Stopped);
        }
        let awake = awake(standing.worked, &read_host(), &asked, now, self.wait);
        if !awake.is_empty() {
            return Some(Checked::Awake(awake));
        }
        self.asking = Some(self.ask(Question::Tick));
        Some(Checked::TickNow)
    }

    /// Ends a check after this instance ticked: waits for the others it asked
    /// to tick, answering any that asks it meanwhile with how it stands now.
    fn end(&mut self, asking: Vec<Asking>, clock: impl Fn() -> u64, stop: &Stop) -> Checked {
        let answering = || self.ticked_again();
        let ticks = Duration::from_secs(TICKS_WITHIN);
        let others = wait_for(asking, ticks, answering, || stop.told());
        if stop.told() {
            return Checked::Stopped;
        }
        let now = clock();
        self.checked = Some(now);
        let standing = self.standing();
        let mut asked = vec![Asked::Answered(standing.clone())];
        asked.extend(others);
        let awake = awake(standing.worked, &read_host(), &asked, now, self.wait);
        if !awake.is_empty() {
            return Checked::Awake(awake);
        }
        match sleep_now() {
            Ok(()) => Checked::Slept,
            Err(error) => Checked::Failed(format!("{error:#}")),
        }
    }

    /// Answers every asker waiting for a tick with how this instance stands:
    /// it ticked within the check it is making.
    fn ticked_again(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let standing = shared.standing.clone();
        for asker in std::mem::take(&mut shared.waiting) {
            let _ = asker.send(standing.clone());
        }
    }

    /// Asks every other instance among its peers `question`, each in a
    /// thread of its own.
    fn ask(&self, question: Question) -> Vec<Asking> {
        let others: Vec<Peer> = self
            .peers
            .open()
            .iter()
            .filter(|peer| !Arc::ptr_eq(&peer.shared, &self.shared))
            .cloned()
            .collect();
        let asked = |peer: Peer| {
            let instance = lock(&peer.shared).standing.instance.clone();
            let answer = std::thread::spawn(move || answer(&peer, question));
            Asking { instance, answer }
        };
        others.into_iter().map(asked).collect()
    }
}

impl Drop for HostSleep {
    fn drop(&mut self) {
        self.peers.open().retain(|peer| !Arc::ptr_eq(&peer.shared, &self.shared));
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What `peer` answers `question`: how it stands; asked to tick first, how
/// it stands once it ticked, or nothing where it did not within
/// `TICKS_WITHIN`.
fn answer(peer: &Peer, question: Question) -> Option<Standing> {
    match question {
        Question::Standing => Some(lock(&peer.shared).standing.clone()),
        Question::Tick => {
            let (send, ticked): (Sender<Standing>, Receiver<Standing>) = channel();
            lock(&peer.shared).waiting.push(send);
            peer.waker.wake();
            ticked.recv_timeout(Duration::from_secs(TICKS_WITHIN)).ok()
        }
    }
}

/// Waits up to `within` for every one of `asking` to answer, calling `meanwhile`
/// as it waits, and no longer once `given_up` → each as asking it came out:
/// one that has not answered by then is silent.
fn wait_for(
    asking: Vec<Asking>,
    within: Duration,
    meanwhile: impl Fn(),
    given_up: impl Fn() -> bool,
) -> Vec<Asked> {
    let started = Instant::now();
    while started.elapsed() < within
        && !given_up()
        && asking.iter().any(|one| !one.answer.is_finished())
    {
        meanwhile();
        std::thread::sleep(Duration::from_millis(20));
    }
    meanwhile();
    let came_out = |one: Asking| match one.answer.is_finished() {
        true => match one.answer.join().ok().flatten() {
            Some(standing) => Asked::Answered(standing),
            None => Asked::Silent { instance: one.instance },
        },
        false => Asked::Silent { instance: one.instance },
    };
    asking.into_iter().map(came_out).collect()
}

/// What a check reads of the host.
fn read_host() -> Host {
    Host { woke: woke().ok(), untouched: untouched().ok() }
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
    use crate::wake::Wakes;

    fn standing(instance: &str) -> Standing {
        Standing {
            instance: instance.into(),
            repo: "o/r".into(),
            version: crate::VERSION.into(),
            no_host_sleep: false,
            idle: true,
            since: 0,
            worked: None,
        }
    }

    /// Each one asked, and what it answered: its instance, where it did.
    fn answers(asking: Vec<Asking>) -> Vec<(String, Option<String>)> {
        let answered = |one: Asking| {
            let answer = one.answer.join().unwrap();
            (one.instance, answer.map(|standing| standing.instance))
        };
        asking.into_iter().map(answered).collect()
    }

    #[test]
    fn an_instance_asks_its_peers_alone_and_one_asked_to_tick_is_woken_and_answers_once_it_ticked()
    {
        let peers = Peers::default();
        let (alpha_wakes, beta_wakes) = (Wakes::default(), Wakes::default());
        let alpha = HostSleep::open(standing("alpha"), alpha_wakes.waker(), &peers, 0);
        let beta = HostSleep::open(standing("beta"), beta_wakes.waker(), &peers, 0);
        let other =
            HostSleep::open(standing("gamma"), Wakes::default().waker(), &Peers::default(), 0);
        let beta_answered = vec![("beta".to_string(), Some("beta".to_string()))];
        assert_eq!(answers(alpha.ask(Question::Standing)), beta_answered, "not itself");
        assert!(alpha_wakes.rest(Duration::ZERO).is_none(), "asked how it stands, none ticks");
        assert!(beta_wakes.rest(Duration::ZERO).is_none());
        assert!(answers(other.ask(Question::Standing)).is_empty(), "another runner's are not");

        // asked to tick: woken, and answered once it ticked, as it stands then
        let asking = alpha.ask(Question::Tick);
        assert!(beta_wakes.rest(Duration::from_secs(5)).is_some(), "beta is woken");
        beta.ticked(100, Some((false, true)));
        let answer = asking.into_iter().next().unwrap().answer.join().unwrap().unwrap();
        assert_eq!((answer.idle, answer.since, answer.worked), (false, 100, Some(100)));

        drop(beta);
        assert!(answers(alpha.ask(Question::Standing)).is_empty(), "one dropped is asked no more");
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

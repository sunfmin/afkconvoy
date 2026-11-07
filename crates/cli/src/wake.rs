//! The wake's two ends: the wakes a fleet instance hears, kept for it by the
//! host runner that holds it, and `afkconvoy wake`, which a worker runs to
//! end that sleep early.
//!
//! A wake reaches the host runner on its socket (`runner_socket`), naming
//! its checkout — any worktree of the repo, a worker's among them — and what
//! sent it, in words (`park #46`), or nothing. The host runner hears it for
//! the fleet instance of that repo (`Wakes::hear`) and answers at once what
//! that instance will do of it (`Answer`). The host runner's own threads
//! wake an instance the same way, with no socket (`Waker`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use afkconvoy_core::wake::Answer;
use anyhow::Result;
use serde_json::{Value, json};

use crate::git::Repo;
use crate::runner_socket::{self, Reached, Request};

/// The host's wake routes for blockers, rebuilt from each fleet's open
/// issues. They hold no fleet state: a closure only asks a peer to read
/// GitHub sooner. A clone belongs to the same host runner.
#[derive(Clone, Default)]
pub struct Blockers {
    routes: Arc<Mutex<BTreeMap<String, Waiting>>>,
}

struct Waiting {
    tracker: String,
    blockers: BTreeSet<u64>,
    wakes: Wakes,
    /// Closures heard while this fleet reads GitHub. Applying these to the
    /// new routes prevents an overlapping, older read from losing a wake.
    reading: Option<BTreeMap<(String, u64), String>>,
}

/// One running fleet's routes, removed when its loop ends or panics.
pub struct Watching {
    host: Blockers,
    instance: String,
}

impl Blockers {
    fn routes(&self) -> MutexGuard<'_, BTreeMap<String, Waiting>> {
        self.routes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn watch(&self, instance: &str, wakes: &Wakes) -> Watching {
        self.routes().insert(
            instance.to_string(),
            Waiting {
                tracker: String::new(),
                blockers: BTreeSet::new(),
                wakes: wakes.clone(),
                reading: None,
            },
        );
        Watching { host: self.clone(), instance: instance.to_string() }
    }
}

impl Watching {
    /// Starts a read. Closures heard during it are kept until `saw`, even
    /// of a newly said tracker whose name this read has not returned yet.
    pub fn begins(&self) {
        let mut routes = self.host.routes();
        let Some(mine) = routes.get_mut(&self.instance) else { return };
        mine.reading = Some(BTreeMap::new());
    }

    /// Replaces this fleet's routes with the open blockers its tick read,
    /// and wakes peers on the same tracker for every closure the tick read
    /// or made. A route is used up by its wake, so seeing a closed blocker
    /// again starts no stream of empty ticks. The woken tick decides what
    /// is unblocked, from GitHub alone.
    pub fn saw(&self, tracker: &str, blockers: impl IntoIterator<Item = u64>, closed: &[u64]) {
        let mut routes = self.host.routes();
        let Some(mine) = routes.get_mut(&self.instance) else { return };
        let tracker = tracker.to_ascii_lowercase();
        mine.tracker.clone_from(&tracker);
        mine.blockers = blockers.into_iter().collect();
        let mut wakes = vec![];
        for ((of, issue), by) in mine.reading.take().unwrap_or_default() {
            if of == tracker && mine.blockers.remove(&issue) {
                wakes.push((mine.wakes.clone(), by));
            }
        }
        for issue in closed {
            let by = format!("{tracker}#{issue} closed, seen by fleet instance {}", self.instance);
            for (instance, peer) in routes.iter_mut() {
                if *instance == self.instance {
                    continue;
                }
                if let Some(reading) = &mut peer.reading {
                    reading.entry((tracker.clone(), *issue)).or_insert_with(|| by.clone());
                }
                if peer.tracker == tracker && peer.blockers.remove(issue) {
                    wakes.push((peer.wakes.clone(), by.clone()));
                }
            }
        }
        // Wakes can write a tick log. Never hold the host's routes while
        // calling into another fleet.
        drop(routes);
        for (wake, by) in wakes {
            wake.hear(Some(by));
        }
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.host.routes().remove(&self.instance);
    }
}

/// What a fleet instance is told of a wake heard during tick `n`: the first
/// of that tick's, as it is heard, with what it named.
pub type During = Box<dyn Fn(u64, Option<String>) + Send>;

/// The most bytes of what a wake names that are heard.
const NAMED_UP_TO: usize = 256;

/// The wakes one fleet instance hears, whether it sleeps or ticks: made by
/// the host runner as it starts the instance, heard on its socket and from
/// its own threads, and read by the instance's loop. A clone is the same.
#[derive(Clone, Default)]
pub struct Wakes {
    heard: Arc<Heard>,
}

/// What those who wake an instance and its loop share.
#[derive(Default)]
struct Heard {
    state: Mutex<State>,
    rung: Condvar,
}

#[derive(Default)]
struct State {
    /// A wake was heard since the last was forgotten, or ended a rest.
    waiting: bool,
    /// What the first of those that named something named.
    by: Option<String>,
    /// The tick running, if one is.
    ticking: Option<u64>,
    /// Whether a wake was heard during it, and told.
    told: bool,
    /// The last tick to have started: none before the first.
    last: u64,
    /// Whom a wake heard during a tick is told, once the loop said.
    during: Option<During>,
}

impl Wakes {
    fn state(&self) -> MutexGuard<'_, State> {
        self.heard.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Tells `during` of the first wake heard during each tick from now on:
    /// the loop says so as it starts.
    pub fn telling(&self, during: During) {
        self.state().during = Some(during);
    }

    /// Hears a wake naming `by`, where it names anything → what the instance
    /// will do of it: the tick it starts, or the tick in progress it
    /// follows. The first heard during a tick is told (`telling`) as it is
    /// heard — under the lock, so the tick's end is said after it.
    pub fn hear(&self, by: Option<String>) -> Answer {
        let by = by.map(|by| named(&by)).filter(|by| !by.is_empty());
        let mut wakes = self.state();
        wakes.waiting = true;
        if wakes.by.is_none() {
            wakes.by.clone_from(&by);
        }
        let answer = Answer::of(wakes.ticking, wakes.last);
        if let Some(count) = wakes.ticking.filter(|_| !wakes.told) {
            wakes.told = true;
            if let Some(during) = &wakes.during {
                during(count, by);
            }
        }
        self.heard.rung.notify_all();
        answer
    }

    /// Forgets every wake heard so far, as tick `count` is about to start.
    /// They carry nothing, so there is nothing to keep: the tick reads GitHub
    /// itself. One heard from now until `ended` is told (`telling`).
    pub fn forget(&self, count: u64) {
        let mut wakes = self.state();
        (wakes.waiting, wakes.by, wakes.ticking, wakes.told, wakes.last) =
            (false, None, Some(count), false, count);
    }

    /// The tick `forget` named ended: a wake heard from now is told by the
    /// rest it ends, not as one during a tick.
    pub fn ended(&self) {
        self.state().ticking = None;
    }

    /// Sleeps for `sleep`, or until a wake — one that arrived before the
    /// sleep began ends it at once → the wake that ended it, if one did,
    /// which is used up.
    pub fn rest(&self, sleep: Duration) -> Option<Woken> {
        let wakes = self.state();
        let (mut wakes, _) = self
            .heard
            .rung
            .wait_timeout_while(wakes, sleep, |wakes| !wakes.waiting)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let by = wakes.by.take();
        std::mem::take(&mut wakes.waiting).then_some(Woken { by })
    }

    /// Whether a wake heard waits, used up by neither a rest nor the start of
    /// a tick: the next rest ends at once.
    pub fn heard(&self) -> bool {
        self.state().waiting
    }

    /// The handle a thread of the host runner wakes this instance with.
    pub fn waker(&self) -> Waker {
        Waker { wakes: self.clone() }
    }
}

/// `by` cut to the bytes a wake names, on a character's edge.
fn named(by: &str) -> String {
    let by = by.trim();
    let mut end = by.len().min(NAMED_UP_TO);
    while !by.is_char_boundary(end) {
        end -= 1;
    }
    by[..end].to_string()
}

/// A wake that ended a rest.
#[derive(Debug, PartialEq)]
pub struct Woken {
    /// What the first wake heard since the last was used up named, where one
    /// named something.
    pub by: Option<String>,
}

/// Wakes one fleet instance from a thread of the process that holds it: a
/// wake that names nothing.
#[derive(Clone)]
pub struct Waker {
    wakes: Wakes,
}

impl Waker {
    pub fn wake(&self) {
        self.wakes.hear(None);
    }
}

/// How long a wake waits for the host runner's answer.
pub(crate) const ANSWER_WITHIN: Duration = Duration::from_secs(3);

/// Asks this host's runner to have the fleet instance of the repo `repo` is
/// a checkout of open its next tick now, naming `by` — what woke it, `park
/// #46` — for it to log: what every command that acts does as it ends
/// (`crate::acting`), `afkconvoy wake` among them. It waits a few seconds at
/// most (`ANSWER_WITHIN`) for the answer: the tick the wake starts, or the
/// tick in progress it follows. With no host runner, or none holding a fleet
/// instance of the repo, or none answering, it does nothing, and whatever
/// goes wrong is not an error: the fleet finds the outcome at its next tick
/// anyway.
pub fn wake(repo: &Repo, by: &str) -> Result<Value> {
    let by = Some(by.to_string()).filter(|by| !by.is_empty());
    let asked = Request::Wake { checkout: repo.dir.clone(), by };
    Ok(answered(runner_socket::ask(&asked, ANSWER_WITHIN), &repo.dir))
}

/// `wake`'s answer, from how asking the host runner of `checkout`'s host came
/// out.
fn answered(reached: Result<Reached>, checkout: &Path) -> Value {
    let not = |why: String| json!({"woke": false, "detail": why});
    let answer = match reached {
        Ok(Reached::Answer(answer)) => answer,
        Ok(Reached::Nobody) => {
            return not(format!(
                "no host runner runs on this host, so no fleet instance heard it: start one with \
                 `afkconvoy run {}`",
                checkout.display()
            ));
        }
        Ok(Reached::Closed) => {
            return not("the host runner ended before it answered: its log, host-runner.log in \
                        the cache directory, says why"
                .to_string());
        }
        Ok(Reached::Unanswered) => {
            return not(format!(
                "the host runner did not answer within {}s: `afkconvoy doctor` says why",
                ANSWER_WITHIN.as_secs()
            ));
        }
        Err(error) => {
            return not(format!("{error:#}: `afkconvoy doctor` says what is wrong with the fleet"));
        }
    };
    if let Some(error) = answer["error"].as_str() {
        return not(error.to_string());
    }
    match (answer["tick"].as_u64(), answer["after_tick"].as_u64()) {
        (Some(tick), _) => json!({"woke": true, "tick": tick}),
        (None, Some(tick)) => json!({"woke": true, "after_tick": tick}),
        (None, None) => not(format!("the host runner answered no tick: {answer}")),
    }
}

/// What the host runner answers a wake it heard as `answer`.
pub fn answer_json(answer: Answer) -> Value {
    match answer {
        Answer::Starts(tick) => json!({"tick": tick}),
        Answer::Follows(tick) => json!({"after_tick": tick}),
    }
}

/// What `afkconvoy wake --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Once, after the last of the issues you open for the fleet with `gh`, or label: nothing `gh` does wakes the fleet, which otherwise takes them up at its next tick (`afkconvoy flows --help` says how soon).

Once, as a worker's last command: after its PR is on its target branch, or its verdict is posted on the issue. A worker's prompt names the exact command. Every command that changes fleet state wakes the `run` as it ends, so after `land` or `verdict` this one is redundant, and harmless.

## Who runs it

Whoever hands the fleet work: a human, or the agent a human is talking to, after opening issues with `gh`, in a checkout of the repo on the fleet's host. And a worker, in its own worktree. Any worktree or checkout of the repo on the fleet's host reaches the same fleet instance.

## What it reads and writes

It speaks to this host's runner, on its socket in the cache directory (host-runner.sock): one signal for the fleet instance it holds of this repo, naming its checkout, itself, and the issue the worker is on if its environment says one. It waits {const:WAKE_ANSWER_SECONDS} seconds at most for the answer. The signal ends that fleet instance's sleep, and no other's; it carries nothing else, and it writes nothing to GitHub: the tick it starts reads the outcome from GitHub itself.

## What it answers

{"woke": true, "tick"} — no tick was in progress: tick `tick` starts now. The fleet instance's tick log says "tick N: started (woken by wake #12)" of it.

{"woke": true, "after_tick"} — tick `after_tick` is in progress: the next starts as it ends. The fleet instance's tick log says "woken by wake #12 during tick N: tick N+1 follows" of the first such wake.

{"woke": false, "detail"} — no fleet instance heard it. With no host runner on this host, `detail` is "no host runner runs on this host, so no fleet instance heard it: start one with `afkconvoy run <checkout>`". Otherwise the host runner holds no fleet instance of this repo, holds it broken — waiting to be started again — or did not answer within {const:WAKE_ANSWER_SECONDS} seconds: `detail` says which, with the line that fixes it — `afkconvoy run <checkout>`, or ask `afkconvoy doctor`.

It exits 0 with each of them. Errors: only "is not in a git checkout", or git's own words for a checkout it refuses. Failing to wake anybody is not an error, and costs at most the {const:WAKE_ANSWER_SECONDS} seconds it waits.

## What trips you up

- It reports nothing. A worker's outcome is its landed PR or its verdict comment: without one of them on GitHub, a wake only makes the fleet look sooner at a worker that stopped with nothing.
- `woke: false` needs no second `wake`: the fleet finds the outcome, or the issue, at its next tick anyway.
- It reaches a fleet instance of this host's runner only: run it on the host whose `run` works the repo. A fleet driven by `tick` hears nothing."#;

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_closed_blocker_wakes_every_peer_on_its_tracker_once() {
        let host = Blockers::default();
        let wakes: Vec<Wakes> = (0..5).map(|_| Wakes::default()).collect();
        let watchers: Vec<Watching> = wakes
            .iter()
            .enumerate()
            .map(|(i, wakes)| host.watch(&format!("fleet-{i}"), wakes))
            .collect();
        for (i, watch) in watchers.iter().enumerate() {
            watch.begins();
            watch.saw(
                if i == 4 { "acme/elsewhere" } else { "ACME/tracker" },
                if i == 3 { vec![99] } else { vec![7, 8] },
                &[],
            );
        }
        watchers[0].saw("acme/tracker", [7, 8], &[7]);
        let by = "acme/tracker#7 closed, seen by fleet instance fleet-0";
        for i in [1, 2] {
            assert_eq!(wakes[i].rest(Duration::ZERO), Some(Woken { by: Some(by.into()) }));
        }
        for i in [0, 3, 4] {
            assert!(!wakes[i].heard(), "no wake of self, unrelated issues or another tracker");
        }
        watchers[0].saw("acme/tracker", [], &[7]);
        assert!(!wakes[1].heard() && !wakes[2].heard(), "a repeated observation sends none");
        watchers[0].saw("acme/tracker", [], &[8]);
        assert!(wakes[1].heard() && wakes[2].heard(), "the other blocker still has its route");
    }

    #[test]
    fn a_closure_during_an_older_rebuild_is_not_lost_when_its_routes_arrive() {
        let host = Blockers::default();
        let wakes = Wakes::default();
        let source = host.watch("source", &Wakes::default());
        let target = host.watch("target", &wakes);
        source.begins();
        target.begins();
        // The target's first read has not published a route yet.
        source.saw("acme/tracker", [], &[7]);
        assert!(!wakes.heard());
        target.saw("acme/tracker", [7], &[]);
        assert!(wakes.rest(Duration::ZERO).is_some(), "the older read cannot hide the closure");
        target.begins();
        target.saw("acme/tracker", [], &[]);
        source.saw("acme/tracker", [], &[7]);
        assert!(!wakes.heard(), "a fresh read with no edge replaces the route");
        target.begins();
        source.saw("acme/tracker", [], &[7]);
        target.saw("acme/another", [7], &[]);
        source.saw("acme/tracker", [], &[7]);
        assert!(!wakes.heard(), "a tracker change replaces the route");
        drop(target);
        assert_eq!(host.routes().len(), 1, "a stopped or broken fleet leaves no route");
    }

    #[test]
    fn a_wake_ends_the_rest_and_one_before_it_ends_it_at_once() {
        let wakes = Wakes::default();
        // one that arrived during the tick
        wakes.hear(None);
        let started = Instant::now();
        assert!(wakes.rest(Duration::from_secs(30)).is_some());
        assert!(started.elapsed() < Duration::from_secs(5));
        // …and with none, the whole sleep is waited out
        let started = Instant::now();
        assert!(wakes.rest(Duration::from_millis(150)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(wakes.rest(Duration::ZERO).is_none());
        // one heard from another thread while it rests ends the rest then
        let waker = wakes.waker();
        let woke = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            waker.wake();
        });
        let started = Instant::now();
        assert_eq!(wakes.rest(Duration::from_secs(30)), Some(Woken { by: None }));
        assert!(started.elapsed() < Duration::from_secs(5));
        woke.join().unwrap();
    }

    #[test]
    fn the_first_wake_heard_during_a_tick_is_told_as_it_is_heard() {
        let wakes = Wakes::default();
        let (told, during) = std::sync::mpsc::channel();
        wakes.telling(Box::new(move |n, _| told.send(n).unwrap()));
        // between ticks: the rest it ends says so, not this
        assert_eq!(wakes.hear(None), Answer::Starts(1));
        assert!(wakes.rest(Duration::from_secs(30)).is_some());
        assert!(during.try_recv().is_err());
        wakes.forget(7);
        assert_eq!(wakes.hear(None), Answer::Follows(7));
        assert_eq!(during.try_recv(), Ok(7), "as it is heard");
        assert_eq!(wakes.hear(None), Answer::Follows(7), "and a second, as well");
        wakes.ended();
        // …which is kept, not told twice, and ends the rest at once
        assert!(wakes.rest(Duration::from_secs(30)).is_some());
        assert!(during.try_recv().is_err(), "one line a tick");
        wakes.forget(8);
        wakes.ended();
        assert_eq!(wakes.hear(None), Answer::Starts(9));
        assert!(wakes.rest(Duration::from_secs(30)).is_some());
        assert!(during.try_recv().is_err(), "heard after the tick ended");
    }

    #[test]
    fn a_wake_says_what_sent_it_and_the_first_that_named_one_is_kept() {
        let wakes = Wakes::default();
        let (told, during) = std::sync::mpsc::channel();
        wakes.telling(Box::new(move |_, by| told.send(by).unwrap()));
        wakes.hear(None);
        wakes.hear(Some("park #46".into()));
        wakes.hear(Some("land #7".into()));
        let woken = wakes.rest(Duration::from_secs(5));
        assert_eq!(woken, Some(Woken { by: Some("park #46".into()) }));
        // used up with the wake it ended
        wakes.hear(Some("  ".into()));
        assert_eq!(wakes.rest(Duration::from_secs(5)), Some(Woken { by: None }), "blank is none");
        // during a tick: told as it is heard, and kept for the tick after it
        wakes.forget(3);
        wakes.hear(Some("verdict #121".into()));
        assert_eq!(during.try_recv(), Ok(Some("verdict #121".to_string())));
        wakes.ended();
        let woken = wakes.rest(Duration::from_secs(5));
        assert_eq!(woken, Some(Woken { by: Some("verdict #121".into()) }));
        // what it names is cut to a wake's length, never inside a character
        wakes.hear(Some("é".repeat(200)));
        let Some(Woken { by: Some(by) }) = wakes.rest(Duration::ZERO) else { panic!("no wake") };
        assert_eq!(by.len(), NAMED_UP_TO);
    }

    #[test]
    fn forgotten_wakes_end_no_rest_however_many_there_were() {
        let wakes = Wakes::default();
        for _ in 0..3 {
            wakes.hear(None);
        }
        wakes.forget(1);
        assert!(wakes.rest(Duration::from_millis(50)).is_none());
        // three at once are one early tick, then the next turn forgets the rest
        for _ in 0..3 {
            wakes.hear(None);
        }
        assert!(wakes.rest(Duration::from_secs(5)).is_some());
        wakes.forget(2);
        assert!(wakes.rest(Duration::ZERO).is_none());
    }

    #[test]
    fn a_wake_answers_what_the_host_runner_said_and_nothing_heard_is_no_error() {
        let checkout = Path::new("/w");
        let said = |answer: Value| answered(Ok(Reached::Answer(answer)), checkout);
        let host_runner = json!({"pid": 42, "version": "0.6.0"});
        let started = said(json!({"tick": 3, "host_runner": host_runner}));
        assert_eq!(started, json!({"woke": true, "tick": 3}));
        let follows = said(json!({"after_tick": 2, "host_runner": host_runner}));
        assert_eq!(follows, json!({"woke": true, "after_tick": 2}));
        let refused = said(json!({"error": "holds no fleet instance of o/r"}));
        assert_eq!(refused, json!({"woke": false, "detail": "holds no fleet instance of o/r"}));
        for reached in [Reached::Nobody, Reached::Closed, Reached::Unanswered] {
            let answer = answered(Ok(reached), checkout);
            assert_eq!(answer["woke"], false, "{answer}");
            assert!(answer["detail"].as_str().is_some_and(|detail| !detail.is_empty()));
        }
        let nobody = answered(Ok(Reached::Nobody), checkout);
        assert!(nobody["detail"].as_str().unwrap().contains("`afkconvoy run /w`"), "{nobody}");
        assert_eq!(answer_json(Answer::Starts(4)), json!({"tick": 4}));
        assert_eq!(answer_json(Answer::Follows(4)), json!({"after_tick": 4}));
    }
}

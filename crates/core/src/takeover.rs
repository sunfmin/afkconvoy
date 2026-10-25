//! Taking claims this fleet instance does not hold: what licenses a take, and
//! what a takeover by hand comes to.

use flux_rs::attrs::*;

use crate::claims::{Beat, Claim, InstanceId, Owner, RunClaim};
use crate::lease::{LEASE, is_stale, stamp_age, stamp_forgotten, stamp_stale};
use crate::patrol;
use crate::precedent::Ruling;

/// What licenses taking another instance's claim: its heartbeat is past the
/// lease, or the claim is stamped with this host, or a human confirmed it.
/// There is no other way to one — so a live peer's claim on another host is
/// never taken unattended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(lapsed: bool, own_host: bool, confirmed: bool)]
#[invariant(lapsed || own_host || confirmed)]
// The fields are what the invariant is stated on: nothing reads them back.
#[allow(dead_code)]
pub struct Licence {
    #[field(bool[lapsed])]
    lapsed: bool,
    #[field(bool[own_host])]
    own_host: bool,
    #[field(bool[confirmed])]
    confirmed: bool,
}

impl Licence {
    /// The holder's heartbeat, as the remote has it now, is past the lease.
    pub fn lapsed(last_beat: Option<u64>, now: u64) -> Option<Licence> {
        let lapsed = is_stale(last_beat, now);
        if lapsed { Some(Licence { lapsed, own_host: false, confirmed: false }) } else { None }
    }

    /// The claim is stamped with the host this fleet starts on: the human
    /// starting it here is the proof the one before it is dead (docs/adr/0003).
    pub fn own_host(owner: Option<&Owner>, host: &str) -> Option<Licence> {
        let own_host = match owner {
            Some(owner) => !host.is_empty() && owner.host == host,
            None => false,
        };
        if own_host { Some(Licence { lapsed: false, own_host, confirmed: false }) } else { None }
    }

    /// A human asked for this instance's claims, and — where its heartbeat is
    /// still fresh — said so twice.
    pub fn by_hand(last_beat: Option<u64>, now: u64, yes: bool) -> Option<Licence> {
        let lapsed = is_stale(last_beat, now);
        if lapsed || yes { Some(Licence { lapsed, own_host: false, confirmed: yes }) } else { None }
    }
}

/// One claim as a takeover finds it: on an issue, or on a run — a patrol's,
/// the proposal of a precedent. Whoever takes an instance's claims takes
/// both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held<'a> {
    Issue(&'a Claim),
    Run(&'a RunClaim),
}

impl<'a> Held<'a> {
    pub fn owner(&self) -> Option<&'a Owner> {
        match self {
            Held::Issue(claim) => claim.owner.as_ref(),
            Held::Run(run) => run.owner.as_ref(),
        }
    }

    fn is_held_by(&self, instance: &str) -> bool {
        match self.owner() {
            Some(owner) => owner.instance == instance,
            None => false,
        }
    }
}

/// What a human is told the run whose claim is keyed `key` is.
pub fn run_said(key: &str) -> String {
    if let Some(ruling) = Ruling::claimed(key) {
        format!("the proposal of the ruling on #{}", ruling.issue)
    } else if let Some(name) = patrol::claimed(key) {
        format!("the run of the patrol {name}")
    } else if key == crate::cut::CLAIM_KEY {
        "the cut".to_string()
    } else {
        format!("the run {key}")
    }
}

/// Every claim the remote has, in the order a takeover takes them: those on
/// issues, lowest first, then those on runs, by key.
pub fn held<'a>(claims: &'a [Claim], runs: &'a [RunClaim]) -> Vec<Held<'a>> {
    let mut issues: Vec<&Claim> = claims.iter().collect();
    issues.sort_by_key(|claim| claim.number);
    let mut keyed: Vec<&RunClaim> = runs.iter().collect();
    keyed.sort_by(|a, b| a.key.cmp(&b.key));
    let mut all: Vec<Held> = Vec::new();
    for claim in issues {
        all.push(Held::Issue(claim));
    }
    for run in keyed {
        all.push(Held::Run(run));
    }
    all
}

/// The claims a fleet starting as `me` on `host` takes at once: every one of
/// `held` stamped with that host that is not already its own, in its order.
pub fn stamped_with<'a>(
    held: &[Held<'a>],
    me: &InstanceId,
    host: &str,
) -> Vec<(Held<'a>, Licence)> {
    let mut mine: Vec<(Held, Licence)> = Vec::new();
    for claim in held {
        if claim.is_held_by(me.as_str()) {
            continue;
        }
        if let Some(licence) = Licence::own_host(claim.owner(), host) {
            mine.push((*claim, licence));
        }
    }
    mine
}

/// Whether `instance` holds any of `held`: a claim on an issue or on a run.
pub fn holds(held: &[Held], instance: &str) -> bool {
    for claim in held {
        if claim.is_held_by(instance) {
            return true;
        }
    }
    false
}

/// The instances whose heartbeats a fleet starting as `me` on `host` removes:
/// each that holds none of `held` and is dead to it — this start took its
/// claims (`taken_from`), or its heartbeat is stamped with this host and past
/// the lease, or is forgotten, whichever host stamped it. A live peer's is
/// never one, and neither is its own.
pub fn swept_at_start<'a>(
    held: &[Held],
    heartbeats: &'a [(String, Beat)],
    taken_from: &[String],
    me: &InstanceId,
    host: &str,
    now: u64,
) -> Vec<&'a str> {
    let mut swept: Vec<&str> = Vec::new();
    for (instance, beat) in heartbeats {
        if instance == me.as_str() || holds(held, instance) {
            continue;
        }
        let own_host = !host.is_empty() && beat.host == host;
        let dead = taken_from.contains(instance)
            || (own_host && stamp_stale(beat.ts, now))
            || stamp_forgotten(beat.ts, now);
        if dead {
            swept.push(instance);
        }
    }
    swept
}

/// One fleet instance the remote remembers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    /// `None`: claims whose record names nobody.
    pub instance: Option<String>,
    pub host: Option<String>,
    /// The issues it holds a claim on, lowest first.
    pub claims: Vec<u64>,
    /// The runs it holds a claim on, by key.
    pub runs: Vec<String>,
    pub heartbeat: Option<u64>,
    /// How old its heartbeat is; `None` with no heartbeat, or one from the future.
    pub heartbeat_age: Option<u64>,
    /// Its heartbeat is within the lease.
    pub fresh: bool,
    pub is_me: bool,
}

impl Remembered {
    /// How many claims it holds, on issues and on runs.
    pub fn claim_count(&self) -> usize {
        self.claims.len().saturating_add(self.runs.len())
    }
}

/// Every instance the remote remembers — by a claim or by a heartbeat — the
/// ones holding most claims first, on issues and on runs alike, then by id. One with a heartbeat and no
/// claim is listed too: that is a fleet that is idle, and seeing it is how a
/// human tells it from the one that died holding work.
pub fn remembered(
    held: &[Held],
    heartbeats: &[(String, u64)],
    me: Option<&InstanceId>,
    now: u64,
) -> Vec<Remembered> {
    let mut rows: Vec<Remembered> = Vec::new();
    for claim in held {
        let instance = claim.owner().map(|owner| owner.instance.clone());
        let at = match row_of(&rows, &instance) {
            Some(at) => at,
            None => {
                rows.push(row(instance, heartbeats, me, now));
                rows.len() - 1
            }
        };
        match claim {
            Held::Issue(claim) => rows[at].claims.push(claim.number),
            Held::Run(run) => rows[at].runs.push(run.key.clone()),
        }
        if rows[at].host.is_none() {
            let host = claim.owner().map(|owner| owner.host.clone());
            rows[at].host = host.filter(|host| !host.is_empty());
        }
    }
    for (instance, _) in heartbeats {
        let instance = Some(instance.clone());
        if row_of(&rows, &instance).is_none() {
            rows.push(row(instance, heartbeats, me, now));
        }
    }
    rows.sort_by(|a, b| {
        let by_claims = b.claim_count().cmp(&a.claim_count());
        by_claims
            .then(a.instance.is_none().cmp(&b.instance.is_none()))
            .then(a.instance.cmp(&b.instance))
    });
    rows
}

/// Where `instance`'s row is among `rows`; `None` when it has none yet.
#[spec(fn(&[Remembered][@n], &Option<String>) -> Option<usize{v: v < n}>)]
fn row_of(rows: &[Remembered], instance: &Option<String>) -> Option<usize> {
    let mut at = 0;
    while at < rows.len() {
        if rows[at].instance == *instance {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// The row of an instance no claim has been filed under yet.
fn row(
    instance: Option<String>,
    heartbeats: &[(String, u64)],
    me: Option<&InstanceId>,
    now: u64,
) -> Remembered {
    let beat = instance.as_ref().and_then(|instance| {
        heartbeats.iter().find(|(name, _)| name == instance).map(|(_, ts)| *ts)
    });
    Remembered {
        is_me: match (&instance, me) {
            (Some(instance), Some(me)) => instance == me.as_str(),
            _ => false,
        },
        instance,
        host: None,
        claims: vec![],
        runs: vec![],
        heartbeat: beat,
        heartbeat_age: beat.and_then(|beat| stamp_age(beat, now)),
        fresh: !is_stale(beat, now),
    }
}

/// What `takeover --from <target>` comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Takeover<'a> {
    /// Take these claims, in the order they were held in, each under the
    /// licence.
    Take(Vec<Held<'a>>, Licence),
    /// The target's heartbeat is still fresh: it looks alive, and taking its
    /// claims steals live work if the human is wrong. Nothing is taken until
    /// they say so again.
    Confirm { heartbeat_age: Option<u64> },
    /// Nothing to take, and why.
    Nothing(String),
    /// The target is this fleet instance: its claims are already its own.
    Mine,
}

/// Decides a takeover by hand of `target`'s claims by `me`. `yes`: the human
/// confirmed taking from an instance that looks alive.
pub fn plan_takeover<'a>(
    held: &[Held<'a>],
    heartbeats: &[(String, u64)],
    target: &str,
    me: &InstanceId,
    now: u64,
    yes: bool,
) -> Takeover<'a> {
    if target == me.as_str() {
        return Takeover::Mine;
    }
    let mut theirs: Vec<Held> = Vec::new();
    for claim in held {
        if claim.is_held_by(target) {
            theirs.push(*claim);
        }
    }
    let beat = heartbeats.iter().find(|(name, _)| name == target).map(|(_, ts)| *ts);
    if theirs.is_empty() {
        let why = if beat.is_some() {
            "that instance holds no claims (nothing to take)"
        } else {
            "no instance by that id holds any claim"
        };
        return Takeover::Nothing(why.to_string());
    }
    match Licence::by_hand(beat, now, yes) {
        Some(licence) => Takeover::Take(theirs, licence),
        None => Takeover::Confirm { heartbeat_age: beat.and_then(|beat| stamp_age(beat, now)) },
    }
}

/// What a human is told before a takeover that needs their word.
pub fn confirm_detail(heartbeat_age: Option<u64>) -> String {
    let age = match heartbeat_age {
        Some(age) => format!("only {age}s old"),
        None => "stamped from the future".to_string(),
    };
    format!(
        "that fleet's heartbeat is {age} (lease {LEASE}s) — it looks ALIVE; taking its claims \
         steals its live work if you are wrong. Run it again with --yes to take them"
    )
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A live peer's claim on another host, taken with nobody's word for it.
    #[should_fail]
    fn a_live_peers_claim_taken_unattended() -> Licence {
        Licence { lapsed: false, own_host: false, confirmed: false }
    }

    /// Whatever the heartbeat read, taken as lapsed.
    #[should_fail]
    fn a_licence_on_whatever_was_read(lapsed: bool) -> Licence {
        Licence { lapsed, own_host: false, confirmed: false }
    }

    /// A takeover by hand of a fleet that looks alive, on one word.
    #[should_fail]
    fn a_fresh_fleet_taken_without_a_yes(yes: bool) -> Licence {
        Licence { lapsed: false, own_host: false, confirmed: yes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;

    const NOW: u64 = 100_000;

    fn id(text: &str) -> InstanceId {
        InstanceId::parse(text).unwrap()
    }

    fn claim(number: u64, instance: &str, host: &str) -> Claim {
        let owner = Owner { instance: instance.into(), host: host.into(), ts: 1 };
        Claim::read(number, &owner.write(), format!("sha{number}"))
    }

    fn nobody(number: u64) -> Claim {
        Claim::read(number, "", format!("sha{number}"))
    }

    fn run(key: &str, instance: &str, host: &str) -> RunClaim {
        let owner = Owner { instance: instance.into(), host: host.into(), ts: 1 };
        RunClaim { key: key.into(), owner: Some(owner), sha: format!("sha-{key}"), over: false }
    }

    /// What a takeover names each claim by.
    fn names(held: &[Held]) -> Vec<String> {
        let name = |held: &Held| match held {
            Held::Issue(claim) => claim.number.to_string(),
            Held::Run(run) => run.key.clone(),
        };
        held.iter().map(name).collect()
    }

    #[test]
    fn a_lapsed_heartbeat_licenses_a_take_and_a_live_one_does_not() {
        assert!(Licence::lapsed(Some(NOW - LEASE - 1), NOW).is_some());
        assert!(Licence::lapsed(None, NOW).is_some(), "an owner that never beat");
        assert!(Licence::lapsed(Some(NOW - LEASE), NOW).is_none());
        assert!(Licence::lapsed(Some(NOW), NOW).is_none());
    }

    #[test]
    fn a_start_takes_every_claim_stamped_with_its_host_whatever_the_heartbeat() {
        let claims = [
            claim(9, "old", "h1"),
            claim(3, "older", "h1"),
            claim(4, "me", "h1"),
            claim(5, "peer", "h2"),
            claim(6, "peer", ""),
            nobody(7),
        ];
        // A patrol's run and a proposal are claims as an issue's is (#132).
        let runs = [
            run("precedent-7-c2001", "old", "h1"),
            run("patrol-triage", "old", "h1"),
            run("patrol-rounds", "me", "h1"),
            run("patrol-sweep", "peer", "h2"),
        ];
        let all = held(&claims, &runs);
        let taken: Vec<Held> =
            stamped_with(&all, &id("me"), "h1").iter().map(|(claim, _)| *claim).collect();
        assert_eq!(
            names(&taken),
            ["3", "9", "patrol-triage", "precedent-7-c2001"],
            "not my own, not another host's, not nobody's"
        );
        assert!(stamped_with(&all, &id("me"), "").is_empty(), "a host with no name owns nothing");
    }

    #[test]
    fn a_start_sweeps_the_heartbeats_that_are_dead_to_it_and_hold_nothing() {
        use crate::lease::FORGOTTEN;
        let beat = |instance: &str, host: &str, ts: u64| {
            (instance.to_string(), Beat { ts, host: host.into(), sha: format!("sha-{instance}") })
        };
        let stale = NOW - LEASE - 1;
        let beats = [
            beat("me", "h1", stale),
            beat("gone", "h1", stale),
            beat("idle", "h1", NOW - LEASE),
            beat("taken", "h1", NOW - 60),
            beat("kept", "h1", stale),
            beat("peer", "h2", stale),
            beat("old-peer", "h2", NOW - FORGOTTEN - 1),
            beat("old-holder", "h2", NOW - FORGOTTEN - 1),
            beat("hostless", "", stale),
            beat("old-hostless", "", NOW - FORGOTTEN - 1),
        ];
        // What is held once the start took what it could: `kept` won a race.
        let claims = [claim(3, "me", "h1"), claim(4, "kept", "h1")];
        let runs = [run("patrol-triage", "old-holder", "h2")];
        let all = held(&claims, &runs);
        let taken_from = ["taken".to_string(), "kept".to_string()];
        let swept = swept_at_start(&all, &beats, &taken_from, &id("me"), "h1", NOW);
        assert_eq!(swept, ["gone", "taken", "old-peer", "old-hostless"]);
        // A host with no name has no heartbeat of its own to sweep.
        let nameless = swept_at_start(&all, &beats, &[], &id("me"), "", NOW);
        assert_eq!(nameless, ["old-peer", "old-hostless"]);
    }

    #[test]
    fn a_run_taken_is_said_as_what_it_is_a_run_of() {
        assert_eq!(run_said("patrol-triage"), "the run of the patrol triage");
        assert_eq!(run_said("precedent-7-c2001"), "the proposal of the ruling on #7");
        assert_eq!(run_said("cut"), "the cut");
        assert_eq!(run_said("else"), "the run else");
    }

    #[test]
    fn the_remote_remembers_instances_by_claim_and_by_heartbeat() {
        let claims =
            [claim(9, "dead", "h2"), claim(3, "dead", "h2"), claim(4, "me", "h1"), nobody(7)];
        let runs = [run("patrol-triage", "dead", "h2"), run("patrol-rounds", "gone", "h4")];
        let beats = [
            ("dead".to_string(), NOW - LEASE - 100),
            ("me".to_string(), NOW - 10),
            ("idle".to_string(), NOW - 20),
        ];
        let mut rows = remembered(&held(&claims, &runs), &beats, Some(&id("me")), NOW);
        let names: Vec<Option<&str>> = rows.iter().map(|row| row.instance.as_deref()).collect();
        assert_eq!(names, [Some("dead"), Some("gone"), Some("me"), None, Some("idle")]);
        let dead = &rows[0];
        assert_eq!((dead.claims.clone(), dead.host.as_deref()), (vec![3, 9], Some("h2")));
        assert_eq!(dead.runs, ["patrol-triage"]);
        assert_eq!((dead.heartbeat_age, dead.fresh, dead.is_me), (Some(LEASE + 100), false, false));
        // An instance that holds nothing but a run is one that holds a claim.
        let gone = rows.remove(1);
        assert_eq!(
            (gone.claims, gone.runs, gone.host.as_deref()),
            (vec![], vec!["patrol-rounds".to_string()], Some("h4"))
        );
        assert_eq!((rows[1].fresh, rows[1].is_me), (true, true));
        assert_eq!(
            (rows[2].claims.clone(), rows[2].fresh, rows[2].heartbeat),
            (vec![7], false, None)
        );
        assert_eq!((rows[3].claims.len(), rows[3].fresh), (0, true));
    }

    #[test]
    fn a_takeover_by_hand_skips_the_lease_but_asks_twice_of_a_fleet_that_looks_alive() {
        let claims = [claim(9, "peer", "h2"), claim(3, "peer", "h2"), claim(4, "me", "h1")];
        let runs = [run("patrol-triage", "peer", "h2"), run("patrol-rounds", "runner", "h3")];
        let claims = held(&claims, &runs);
        let me = id("me");
        let dead = [("peer".to_string(), NOW - LEASE - 1)];
        let Takeover::Take(taken, _) = plan_takeover(&claims, &dead, "peer", &me, NOW, false)
        else {
            panic!("a lapsed fleet's claims are taken")
        };
        assert_eq!(names(&taken), ["3", "9", "patrol-triage"]);
        // One that holds a run and no issue has a claim to take.
        let Takeover::Take(taken, _) = plan_takeover(&claims, &[], "runner", &me, NOW, false)
        else {
            panic!("a run is a claim")
        };
        assert_eq!(names(&taken), ["patrol-rounds"]);
        assert!(matches!(plan_takeover(&claims, &[], "peer", &me, NOW, false), Takeover::Take(..)));

        let alive = [("peer".to_string(), NOW - 60)];
        let asked = plan_takeover(&claims, &alive, "peer", &me, NOW, false);
        assert_eq!(asked, Takeover::Confirm { heartbeat_age: Some(60) });
        assert!(confirm_detail(Some(60)).contains("only 60s old"));
        assert!(matches!(
            plan_takeover(&claims, &alive, "peer", &me, NOW, true),
            Takeover::Take(..)
        ));

        assert_eq!(plan_takeover(&claims, &alive, "me", &me, NOW, true), Takeover::Mine);
        let Takeover::Nothing(why) = plan_takeover(&claims, &alive, "ghost", &me, NOW, true) else {
            panic!()
        };
        assert!(why.contains("no instance by that id"));
        let idle = [("idle".to_string(), NOW)];
        let Takeover::Nothing(why) = plan_takeover(&claims, &idle, "idle", &me, NOW, true) else {
            panic!()
        };
        assert!(why.contains("holds no claims"));
    }
}

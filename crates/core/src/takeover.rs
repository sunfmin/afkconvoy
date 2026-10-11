//! Taking claims this fleet instance does not hold: what licenses a take, and
//! what a takeover by hand comes to.

use flux_rs::attrs::*;

use crate::claims::{Claim, InstanceId};
use crate::lease::{LEASE, is_stale, stamp_age};

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
    pub fn own_host(claim: &Claim, host: &str) -> Option<Licence> {
        let own_host = match &claim.owner {
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

/// The claims a fleet starting as `me` on `host` takes at once: every one
/// stamped with that host that is not already its own, lowest issue first.
pub fn stamped_with<'a>(
    claims: &'a [Claim],
    me: &InstanceId,
    host: &str,
) -> Vec<(&'a Claim, Licence)> {
    let mut mine: Vec<(&Claim, Licence)> = Vec::new();
    for claim in claims {
        if claim.is_held_by(me) {
            continue;
        }
        if let Some(licence) = Licence::own_host(claim, host) {
            mine.push((claim, licence));
        }
    }
    mine.sort_by_key(|(claim, _)| claim.number);
    mine
}

/// One fleet instance the remote remembers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    /// `None`: claims whose record names nobody.
    pub instance: Option<String>,
    pub host: Option<String>,
    /// The issues it holds a claim on, lowest first.
    pub claims: Vec<u64>,
    pub heartbeat: Option<u64>,
    /// How old its heartbeat is; `None` with no heartbeat, or one from the future.
    pub heartbeat_age: Option<u64>,
    /// Its heartbeat is within the lease.
    pub fresh: bool,
    pub is_me: bool,
}

/// Every instance the remote remembers — by a claim or by a heartbeat — the
/// ones holding most claims first, then by id. One with a heartbeat and no
/// claim is listed too: that is a fleet that is idle, and seeing it is how a
/// human tells it from the one that died holding work.
pub fn remembered(
    claims: &[Claim],
    heartbeats: &[(String, u64)],
    me: Option<&InstanceId>,
    now: u64,
) -> Vec<Remembered> {
    let mut rows: Vec<Remembered> = Vec::new();
    let row_of = |rows: &mut Vec<Remembered>, instance: Option<String>| -> usize {
        if let Some(at) = rows.iter().position(|row| row.instance == instance) {
            return at;
        }
        let beat = instance.as_ref().and_then(|instance| {
            heartbeats.iter().find(|(name, _)| name == instance).map(|(_, ts)| *ts)
        });
        rows.push(Remembered {
            is_me: match (&instance, me) {
                (Some(instance), Some(me)) => instance == me.as_str(),
                _ => false,
            },
            instance,
            host: None,
            claims: vec![],
            heartbeat: beat,
            heartbeat_age: beat.and_then(|beat| stamp_age(beat, now)),
            fresh: !is_stale(beat, now),
        });
        rows.len() - 1
    };
    for claim in claims {
        let at = row_of(&mut rows, claim.owner.as_ref().map(|owner| owner.instance.clone()));
        rows[at].claims.push(claim.number);
        if rows[at].host.is_none() {
            let host = claim.owner.as_ref().map(|owner| owner.host.clone());
            rows[at].host = host.filter(|host| !host.is_empty());
        }
    }
    for (instance, _) in heartbeats {
        row_of(&mut rows, Some(instance.clone()));
    }
    for row in &mut rows {
        row.claims.sort();
    }
    rows.sort_by(|a, b| {
        let by_claims = b.claims.len().cmp(&a.claims.len());
        by_claims
            .then(a.instance.is_none().cmp(&b.instance.is_none()))
            .then(a.instance.cmp(&b.instance))
    });
    rows
}

/// What `takeover --from <target>` comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Takeover<'a> {
    /// Take these claims, lowest issue first, each under the licence.
    Take(Vec<&'a Claim>, Licence),
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
    claims: &'a [Claim],
    heartbeats: &[(String, u64)],
    target: &str,
    me: &InstanceId,
    now: u64,
    yes: bool,
) -> Takeover<'a> {
    if target == me.as_str() {
        return Takeover::Mine;
    }
    let mut theirs: Vec<&Claim> = Vec::new();
    for claim in claims {
        if claim.owner.as_ref().is_some_and(|owner| owner.instance == target) {
            theirs.push(claim);
        }
    }
    theirs.sort_by_key(|claim| claim.number);
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
        Claim { number, owner: Some(owner), sha: format!("sha{number}") }
    }

    fn nobody(number: u64) -> Claim {
        Claim { number, owner: None, sha: format!("sha{number}") }
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
        let taken: Vec<u64> =
            stamped_with(&claims, &id("me"), "h1").iter().map(|(claim, _)| claim.number).collect();
        assert_eq!(taken, [3, 9], "not my own, not another host's, not nobody's");
        assert!(
            stamped_with(&claims, &id("me"), "").is_empty(),
            "a host with no name owns nothing"
        );
    }

    #[test]
    fn the_remote_remembers_instances_by_claim_and_by_heartbeat() {
        let claims =
            [claim(9, "dead", "h2"), claim(3, "dead", "h2"), claim(4, "me", "h1"), nobody(7)];
        let beats = [
            ("dead".to_string(), NOW - LEASE - 100),
            ("me".to_string(), NOW - 10),
            ("idle".to_string(), NOW - 20),
        ];
        let rows = remembered(&claims, &beats, Some(&id("me")), NOW);
        let names: Vec<Option<&str>> = rows.iter().map(|row| row.instance.as_deref()).collect();
        assert_eq!(names, [Some("dead"), Some("me"), None, Some("idle")]);
        let dead = &rows[0];
        assert_eq!((dead.claims.clone(), dead.host.as_deref()), (vec![3, 9], Some("h2")));
        assert_eq!((dead.heartbeat_age, dead.fresh, dead.is_me), (Some(LEASE + 100), false, false));
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
        let me = id("me");
        let dead = [("peer".to_string(), NOW - LEASE - 1)];
        let Takeover::Take(taken, _) = plan_takeover(&claims, &dead, "peer", &me, NOW, false)
        else {
            panic!("a lapsed fleet's claims are taken")
        };
        assert_eq!(taken.iter().map(|claim| claim.number).collect::<Vec<_>>(), [3, 9]);
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

//! Taking claims this fleet instance does not hold: the stale ones a tick
//! reclaims, the ones stamped with this host that a start takes, and a
//! takeover by hand.

use afkconvoy_core::claims::Claim;
use afkconvoy_core::takeover::{
    Licence, Remembered, Takeover, confirm_detail, plan_takeover, remembered, stamped_with,
};
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::advance::advance;
use crate::claims::{Me, Take, delete, take};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::stage::Flows;

/// Puts `claim` in my name under `_licence` — the one way a claim is taken
/// from another instance — with one atomic push the remote rejects unless the
/// ref still points at the sha that was read → whether it was won. It is
/// still the claim it was created as: what was declared under it stands.
fn take_over(fleet: &mut Fleet, me: &Me, claim: &Claim, _licence: &Licence) -> Result<bool> {
    let taken = take(&fleet.repo, &mut fleet.scan, me, claim.number, Take::From(claim))?;
    Ok(taken["won"] == true)
}

/// The claim on `issue` as the scan has it, with the licence its holder's
/// heartbeat gives now; `None` when it is gone, mine, or its holder is alive.
fn lapsed(fleet: &Fleet, me: &Me, issue: u64) -> Option<(Claim, Licence)> {
    let claim = fleet.scan.claim_of(issue).filter(|claim| !claim.is_held_by(&me.instance))?;
    let holder = claim.owner.as_ref().map(|owner| owner.instance.as_str());
    let beat = holder.and_then(|holder| fleet.scan.heartbeats.get(holder).copied());
    Some((claim.clone(), Licence::lapsed(beat, fleet.now)?))
}

/// The tick's take of one stale claim: taken, then continued from what
/// survived — or, where a stage had already declared under it, advanced as
/// that verdict says, as `flows` read it. `Lost` when a peer took it first,
/// or its holder beat again.
pub fn reclaim(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    flows: &Flows,
    issue: u64,
) -> Result<Outcome> {
    let Some((claim, licence)) = lapsed(fleet, me, issue) else { return Ok(Outcome::Lost) };
    if !take_over(fleet, me, &claim, &licence)? {
        return Ok(Outcome::Lost);
    }
    if fleet.config.bound.is_some() && flows.called(issue, &claim.id)?.said.is_some() {
        advance(fleet, me, agent, flows, issue)?;
    } else {
        dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
    }
    Ok(Outcome::Done)
}

/// The tick's delete of one stale claim on a closed issue — only while the
/// ref still points at the sha the rebuild reported.
pub fn unlock(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Outcome> {
    let Some((claim, _licence)) = lapsed(fleet, me, issue) else { return Ok(Outcome::Lost) };
    delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
    Ok(Outcome::Done)
}

/// What a start took: the claims stamped with this host, whatever their
/// holder's heartbeat says (docs/adr/0003). The next tick continues each.
pub fn at_start(fleet: &mut Fleet, me: &Me) -> Result<Value> {
    let claims = fleet.scan.claims.clone();
    let mut taken = vec![];
    let mut lost = vec![];
    for (claim, licence) in stamped_with(&claims, &me.instance, &me.host) {
        if take_over(fleet, me, claim, &licence)? {
            taken.push(claim.number);
        } else {
            lost.push(claim.number);
        }
    }
    Ok(json!({"taken": taken, "lost": lost}))
}

fn row(remembered: &Remembered) -> Value {
    json!({
        "instance": remembered.instance,
        "host": remembered.host,
        "claims": remembered.claims,
        "claim_count": remembered.claims.len(),
        "heartbeat_ts": remembered.heartbeat,
        "heartbeat_age": remembered.heartbeat_age,
        "fresh": remembered.fresh,
        "is_me": remembered.is_me,
    })
}

fn heartbeats(fleet: &Fleet) -> Vec<(String, u64)> {
    fleet.scan.heartbeats.iter().map(|(instance, ts)| (instance.clone(), *ts)).collect()
}

/// `afkconvoy takeover --list`: every instance the remote remembers.
pub fn list(fleet: &Fleet, me: Option<&Me>) -> Value {
    let me = me.map(|me| &me.instance);
    let rows = remembered(&fleet.scan.claims, &heartbeats(fleet), me, fleet.now);
    json!({"instances": rows.iter().map(row).collect::<Vec<_>>(),
        "me": me.map(|me| me.as_str()), "now": fleet.now,
        "lease": afkconvoy_core::lease::LEASE})
}

/// `afkconvoy takeover --from <id>`: take that instance's claims without
/// waiting for the lease. One whose heartbeat is fresh takes nothing until
/// `yes`. The next tick of `me` continues each claim taken.
pub fn takeover(fleet: &mut Fleet, me: &Me, from: &str, yes: bool) -> Result<Value> {
    let claims = fleet.scan.claims.clone();
    match plan_takeover(&claims, &heartbeats(fleet), from, &me.instance, fleet.now, yes) {
        Takeover::Mine => {
            bail!("{from} is this fleet's own instance id: its claims are already its own")
        }
        Takeover::Nothing(detail) => {
            Ok(json!({"action": "none", "instance": from, "taken": [], "lost": [],
                "detail": detail}))
        }
        Takeover::Confirm { heartbeat_age } => {
            Ok(json!({"action": "confirm", "instance": from, "taken": [], "lost": [],
                "heartbeat_age": heartbeat_age, "detail": confirm_detail(heartbeat_age)}))
        }
        Takeover::Take(theirs, licence) => {
            let mut taken = vec![];
            let mut lost = vec![];
            for claim in theirs {
                if take_over(fleet, me, claim, &licence)? {
                    taken.push(claim.number);
                } else {
                    lost.push(claim.number);
                }
            }
            let mut detail = format!("took {} claim(s) from {from}", taken.len());
            if !lost.is_empty() {
                detail.push_str("; a lost one means that fleet is not dead — its ref moved");
            }
            Ok(json!({"action": "taken", "instance": from, "as": me.instance.as_str(),
                "taken": taken, "lost": lost, "detail": detail}))
        }
    }
}

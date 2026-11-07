//! Taking claims this fleet instance does not hold: the stale ones a tick
//! reclaims, the ones stamped with this host that a start takes, and a
//! takeover by hand.

use afkconvoy_core::claims::{Claim, InstanceId};
use afkconvoy_core::takeover::{
    Held, Licence, Remembered, Takeover, confirm_detail, held, plan_takeover, remembered,
    stamped_with, swept_at_start,
};
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::advance::advance;
use crate::claims::{Me, Take, delete, drop_heartbeat, retake_run, take};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::found::Found;
use crate::stage::Flows;

/// Puts `claim` in my name under `_licence` — the one way a claim is taken
/// from another instance — with one atomic push the remote rejects unless the
/// ref still points at the sha that was read → whether it was won. It is
/// still the claim it was created as: what was declared under it stands.
fn take_over(fleet: &mut Fleet, me: &Me, claim: &Claim, _licence: &Licence) -> Result<bool> {
    let taken = take(&fleet.repo, &mut fleet.scan, me, claim.number, Take::From(claim))?;
    Ok(taken["won"] == true)
}

/// What taking some claims came to: the issues and the runs whose claims were
/// taken, and the ones that moved first.
#[derive(Default)]
struct Took {
    taken: Vec<u64>,
    lost: Vec<u64>,
    taken_runs: Vec<String>,
    lost_runs: Vec<String>,
}

impl Took {
    /// Takes `claim` under `licence` (`take_over`), and keeps what came of
    /// it. An issue's is continued by the next tick; a run's is ended by it
    /// once its worker is gone or stopped.
    fn take(&mut self, fleet: &mut Fleet, me: &Me, claim: Held, licence: &Licence) -> Result<()> {
        match claim {
            Held::Issue(claim) => {
                let won = take_over(fleet, me, claim, licence)?;
                if won { &mut self.taken } else { &mut self.lost }.push(claim.number);
            }
            Held::Run(run) => {
                let won = retake_run(&fleet.repo, &mut fleet.scan, me, run)?;
                if won { &mut self.taken_runs } else { &mut self.lost_runs }.push(run.key.clone());
            }
        }
        Ok(())
    }

    fn json(&self) -> Value {
        json!({"taken": self.taken, "lost": self.lost,
            "taken_runs": self.taken_runs, "lost_runs": self.lost_runs})
    }

    /// The answer `action` of a takeover from `from` that came to this.
    fn answer(&self, action: &str, from: &str) -> Value {
        let mut answer = self.json();
        answer["action"] = json!(action);
        answer["instance"] = json!(from);
        answer
    }
}

/// The claim on `issue` as the scan has it, with the licence its holder's
/// heartbeat gives now; `None` when it is gone, mine, or its holder is alive.
fn lapsed(fleet: &Fleet, me: &Me, issue: u64) -> Option<(Claim, Licence)> {
    let claim = fleet.scan.claim_of(issue).filter(|claim| !claim.is_held_by(&me.instance))?;
    let holder = claim.owner.as_ref().map(|owner| owner.instance.as_str());
    let beat = holder.and_then(|holder| fleet.scan.last_beat(holder));
    Some((claim.clone(), Licence::lapsed(beat, fleet.now)?))
}

/// The tick's take of one stale claim: taken, then continued from what
/// survived — or, where a stage had already declared under it, advanced as
/// that verdict says, as `flows` read it. `Lost` when a peer took it first,
/// or its holder beat again.
pub fn reclaim(
    fleet: &mut Fleet,
    found: &mut Found,
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
        advance(fleet, found, me, agent, flows, issue)?;
    } else {
        dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
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

/// What a start took: the claims stamped with this host — on an issue, on a
/// patrol's run or on a cut — whatever their holder's heartbeat says
/// (docs/adr/0003). The next tick continues each issue, and ends each run
/// whose worker is gone or stopped.
///
/// It then removes the heartbeats that are dead to it and hold nothing
/// (`swept_at_start`) — the ones of the instances it just took from among
/// them — and says whose, as `swept`.
pub fn at_start(fleet: &mut Fleet, me: &Me) -> Result<Value> {
    let (claims, runs) = (fleet.scan.claims.clone(), fleet.scan.runs.clone());
    let mut took = Took::default();
    let mut taken_from = vec![];
    for (claim, licence) in stamped_with(&held(&claims, &runs), &me.instance, &me.host) {
        taken_from.extend(claim.owner().map(|owner| owner.instance.clone()));
        took.take(fleet, me, claim, &licence)?;
    }
    let mut answer = took.json();
    answer["swept"] = json!(sweep(fleet, me, &taken_from)?);
    Ok(answer)
}

/// Removes the heartbeats a start as `me` finds dead, having taken the claims
/// of `taken_from` → the instances whose heartbeat is gone.
fn sweep(fleet: &mut Fleet, me: &Me, taken_from: &[String]) -> Result<Vec<String>> {
    let beats: Vec<_> = fleet
        .scan
        .heartbeats
        .iter()
        .map(|(instance, beat)| (instance.clone(), beat.clone()))
        .collect();
    let dead: Vec<InstanceId> = {
        let held = held(&fleet.scan.claims, &fleet.scan.runs);
        let dead = swept_at_start(&held, &beats, taken_from, &me.instance, &me.host, fleet.now);
        // A ref that names no instance is no heartbeat of the fleet's to remove.
        dead.into_iter().filter_map(|instance| InstanceId::parse(instance).ok()).collect()
    };
    let mut swept = vec![];
    for instance in dead {
        if drop_heartbeat(&fleet.repo, &mut fleet.scan, &instance)? {
            swept.push(instance.to_string());
        }
    }
    Ok(swept)
}

fn row(remembered: &Remembered) -> Value {
    json!({
        "instance": remembered.instance,
        "host": remembered.host,
        "claims": remembered.claims,
        "runs": remembered.runs,
        "claim_count": remembered.claim_count(),
        "heartbeat_ts": remembered.heartbeat,
        "heartbeat_age": remembered.heartbeat_age,
        "fresh": remembered.fresh,
        "is_me": remembered.is_me,
    })
}

/// `afkconvoy takeover --list`: every instance the remote remembers.
pub fn list(fleet: &Fleet, me: Option<&Me>) -> Value {
    let me = me.map(|me| &me.instance);
    let all = held(&fleet.scan.claims, &fleet.scan.runs);
    let rows = remembered(&all, &fleet.scan.beats(), me, fleet.now);
    json!({"instances": rows.iter().map(row).collect::<Vec<_>>(),
        "me": me.map(|me| me.as_str()), "now": fleet.now,
        "lease": afkconvoy_core::lease::LEASE})
}

/// `afkconvoy takeover --from <id>`: take that instance's claims — on issues
/// and on runs — without waiting for the lease. One whose heartbeat is fresh
/// takes nothing until `yes`. The next tick of `me` continues each issue
/// taken, and ends each run.
pub fn takeover(fleet: &mut Fleet, me: &Me, from: &str, yes: bool) -> Result<Value> {
    let (claims, runs) = (fleet.scan.claims.clone(), fleet.scan.runs.clone());
    let all = held(&claims, &runs);
    let mut took = Took::default();
    match plan_takeover(&all, &fleet.scan.beats(), from, &me.instance, fleet.now, yes) {
        Takeover::Mine => {
            bail!("{from} is this fleet's own instance id: its claims are already its own")
        }
        Takeover::Nothing(detail) => {
            let mut answer = took.answer("none", from);
            answer["detail"] = json!(detail);
            Ok(answer)
        }
        Takeover::Confirm { heartbeat_age } => {
            let mut answer = took.answer("confirm", from);
            answer["heartbeat_age"] = json!(heartbeat_age);
            answer["detail"] = json!(confirm_detail(heartbeat_age));
            Ok(answer)
        }
        Takeover::Take(theirs, licence) => {
            for claim in theirs {
                took.take(fleet, me, claim, &licence)?;
            }
            let count = took.taken.len() + took.taken_runs.len();
            let mut detail = format!("took {count} claim(s) from {from}");
            if !took.lost.is_empty() || !took.lost_runs.is_empty() {
                detail.push_str("; a lost one means that fleet is not dead — its ref moved");
            }
            let mut answer = took.answer("taken", from);
            answer["as"] = json!(me.instance.as_str());
            answer["detail"] = json!(detail);
            Ok(answer)
        }
    }
}

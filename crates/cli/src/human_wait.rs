//! A human wait belongs to one host's watch. Doctor only describes its
//! claim; the runner takes it before queueing and rechecks it before sending.

use std::collections::BTreeSet;

use afkconvoy_core::claims::{Claim, InstanceId, wait_ref};
use afkconvoy_core::lease::is_stale;
use anyhow::Result;
use serde_json::Value;

use crate::claims::{Me, Scan, Take};
use crate::git::Repo;
use crate::remote::Fresh;

/// Takes unheld or stale waits, keeps this host's existing ones, and releases
/// its claims whose waits ended. The caller must have read all current waits
/// successfully: an unread checkout is never an empty set of waits.
pub fn reconcile(repo: &Repo, scan: &mut Scan, me: &Me, waiting: &BTreeSet<u64>) -> Result<()> {
    let mut ended = BTreeSet::new();
    for claim in scan.waits.clone() {
        if claim.owner.as_ref().is_some_and(|o| o.host == me.host)
            && !waiting.contains(&claim.number)
        {
            ended.extend(claim.owner.as_ref().map(|o| o.instance.clone()));
            crate::claims::drop_wait(repo, scan, &claim)?;
        }
    }
    for &issue in waiting {
        let previous = scan.waits.iter().find(|c| c.number == issue).cloned();
        match previous {
            Some(claim) if claim.owner.as_ref().is_some_and(|o| o.host == me.host) => (),
            Some(claim) if live(scan, &claim, me.now) => (),
            Some(claim) => crate::claims::take_wait(repo, scan, me, issue, Take::From(&claim))?,
            None => crate::claims::take_wait(repo, scan, me, issue, Take::Fresh)?,
        }
    }
    for instance in ended {
        if scan.waits.iter().any(|c| c.owner.as_ref().is_some_and(|o| o.instance == instance))
            || afkconvoy_core::takeover::holds(
                &afkconvoy_core::takeover::held(&scan.claims, &scan.runs),
                &instance,
            )
        {
            continue;
        }
        let instance = InstanceId::parse(&instance).map_err(anyhow::Error::msg)?;
        crate::claims::drop_heartbeat(repo, scan, &instance)?;
    }
    renew(repo, scan, &me.host, me.now)
}

fn live(scan: &Scan, claim: &Claim, now: u64) -> bool {
    let beat = claim.owner.as_ref().and_then(|o| scan.last_beat(&o.instance));
    !is_stale(beat, now)
}

/// Live holders from the scan doctor already read; no second remote read.
pub fn holders(scan: &Scan, now: u64) -> std::collections::BTreeMap<u64, String> {
    scan.waits
        .iter()
        .filter(|c| live(scan, c, now))
        .filter_map(|c| Some((c.number, c.owner.as_ref()?.host.clone())))
        .collect()
}

/// A wait alone keeps its owner's heartbeat alive, independently of worker
/// claims, watch reports, and how often this host chooses to look.
fn renew(repo: &Repo, scan: &mut Scan, host: &str, now: u64) -> Result<()> {
    let instances: BTreeSet<_> = scan
        .waits
        .iter()
        .filter_map(|c| c.owner.as_ref())
        .filter(|o| o.host == host)
        .map(|o| o.instance.clone())
        .collect();
    for instance in instances {
        let instance = InstanceId::parse(&instance).map_err(anyhow::Error::msg)?;
        crate::claims::beat(repo, scan, &Me { instance, host: host.into(), now })?;
    }
    Ok(())
}

/// Renew even while a long watch round runs or the next look is far away.
pub fn heartbeat() -> Result<()> {
    let host = crate::host_settings::host_name();
    for entry in crate::checkout_list::List::of_host()?.read()?.checkouts {
        let repo = Repo::at(&entry.checkout)?;
        let mut scan = Scan::read(&repo)?;
        renew(&repo, &mut scan, &host, crate::clock())?;
    }
    Ok(())
}

/// Claim before observing the look: losing a race must never reserve an
/// episode or give that wait to a round. Host findings remain local.
pub fn take_look(look: &mut Value) -> Result<()> {
    let host = crate::host_settings::host_name();
    let me = Me {
        instance: InstanceId::mint("watch", uuid::Uuid::new_v4().as_u128() as u32),
        host: host.clone(),
        now: crate::clock(),
    };
    for checkout in look["checkouts"].as_array_mut().into_iter().flatten() {
        let Some(at) = checkout["checkout"].as_str() else { continue };
        if !checkout["doctor"]["waiting_on_human"].is_array() {
            continue;
        }
        let repo = Repo::at(std::path::Path::new(at))?;
        let waiting = checkout["doctor"]["waiting_on_human"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w["issue"].as_u64())
            .collect();
        let mut scan = Scan::read(&repo)?;
        reconcile(&repo, &mut scan, &me, &waiting)?;
        // Losing the claim race never permits queueing, even if its new
        // heartbeat cannot yet be read. Only ownership here permits it.
        let holders: std::collections::BTreeMap<_, _> = scan
            .waits
            .iter()
            .map(|claim| {
                (
                    claim.number,
                    claim.owner.as_ref().map(|o| o.host.as_str()).unwrap_or("an unknown host"),
                )
            })
            .collect();
        let mut taken = BTreeSet::new();
        for wait in checkout["doctor"]["waiting_on_human"].as_array_mut().into_iter().flatten() {
            let holder = wait["issue"]
                .as_u64()
                .and_then(|n| holders.get(&n))
                .copied()
                .unwrap_or("no host (claim not won)");
            let holder = (holder != host).then_some(holder);
            wait["taken_by"] = serde_json::json!(holder);
            if holder.is_some() {
                let identity: crate::doctor::HumanWait = serde_json::from_value(wait.clone())?;
                taken.insert(identity.watch_id());
            }
        }
        if let Some(findings) = checkout["doctor"]["findings"].as_array_mut() {
            findings.retain(|f| f["watch_id"].as_str().is_none_or(|id| !taken.contains(id)));
        }
    }
    Ok(())
}

/// Checks a durable condition at the moment it is sent. The condition's
/// structured identity names its checkout and human wait; host conditions
/// have no shared claim. An absent claim gives no authority to send.
pub fn lost(condition: &str) -> Result<Option<String>> {
    let Some(scoped) = condition.strip_prefix("finding:") else { return Ok(None) };
    let Some(at) = scoped.rfind(":[\"human\",") else { return Ok(None) };
    let identity: Value = serde_json::from_str(&scoped[at + 1..])?;
    let issue = identity[1]["issue"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("human condition has no issue"))?;
    let repo = Repo::at(std::path::Path::new(&scoped[..at]))?;
    let tracker = crate::checkout_list::tracker_of(&repo)?;
    let name = wait_ref(repo.layout(), &tracker, issue);
    let owner = repo
        .remote()
        .records(&[&name], Fresh::Live)?
        .pop()
        .and_then(|r| afkconvoy_core::claims::Owner::read(&r.subject));
    match owner {
        Some(owner) if owner.host == crate::host_settings::host_name() => Ok(None),
        Some(owner) => Ok(Some(owner.host)),
        None => Ok(Some("no host (the claim ended)".into())),
    }
}

/// The discarded item and its current holder remain findable after restart.
pub fn log_drop(condition: &str, holder: &str, action: &str) -> Result<()> {
    crate::host_runner::note(&format!("human wait {condition}: dropped {action}; held by {holder}"))
}

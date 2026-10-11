//! Claims and heartbeats on the remote's refs: the scan that reads them and
//! the pushes that write them.

use std::collections::BTreeMap;

use afkconvoy_core::claims::{
    Claim, ClaimStep, InstanceId, NAMESPACE, Owner, claim_ref, claim_steps, heartbeat_ref,
    read_heartbeat, releasable, write_heartbeat,
};
use afkconvoy_core::record::fleet_number;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::git::{REMOTE, Repo};

/// Who is acting, and when.
pub struct Me {
    pub instance: InstanceId,
    pub host: String,
    pub now: u64,
}

/// Every claim and heartbeat the remote has, as one fetch saw them. A write
/// this process makes is carried into it, so a read after a write sees it.
#[derive(Debug, Default)]
pub struct Scan {
    pub claims: Vec<Claim>,
    pub heartbeats: BTreeMap<String, u64>,
}

impl Scan {
    /// Reads every claim and heartbeat record off the remote.
    /// An error when the remote cannot be read: a fleet whose claims are
    /// unreadable must not look like one holding none.
    pub fn read(repo: &Repo) -> Result<Scan> {
        let mut scan = Scan::default();
        for record in repo.remote_records(&format!("{NAMESPACE}/*"))? {
            let Some(kept) = record.name.strip_prefix(NAMESPACE) else { continue };
            if let Some(name) = kept.strip_prefix("/claim/") {
                if let Some(number) = fleet_number(name) {
                    let owner = Owner::read(&record.subject);
                    scan.claims.push(Claim { number, owner, sha: record.sha });
                }
            } else if let Some(instance) = kept.strip_prefix("/heartbeat/")
                && let Some(ts) = read_heartbeat(&record.subject)
            {
                scan.heartbeats.insert(instance.to_string(), ts);
            }
        }
        scan.claims.sort_by_key(|claim| claim.number);
        Ok(scan)
    }

    pub fn claim_of(&self, number: u64) -> Option<&Claim> {
        self.claims.iter().find(|claim| claim.number == number)
    }

    fn written(&mut self, number: u64, claim: Option<Claim>) {
        self.claims.retain(|claim| claim.number != number);
        self.claims.extend(claim);
        self.claims.sort_by_key(|claim| claim.number);
    }

    pub fn json(&self) -> Value {
        json!({
            "claims": self.claims.iter().map(claim_json).collect::<Vec<_>>(),
            "heartbeats": self.heartbeats,
        })
    }
}

pub fn claim_json(claim: &Claim) -> Value {
    let owner = claim.owner.as_ref();
    json!({
        "number": claim.number,
        "instance": owner.map(|o| &o.instance),
        "host": owner.map(|o| &o.host),
        "ts": owner.map(|o| o.ts),
        "sha": claim.sha,
    })
}

/// Refreshes my heartbeat ref, whatever its age.
fn push_heartbeat(repo: &Repo, scan: &mut Scan, me: &Me) -> Result<()> {
    let sha = repo.record_commit(&write_heartbeat(&me.instance, me.now))?;
    let refname = heartbeat_ref(&me.instance);
    repo.git(&["push", "--quiet", "--force", REMOTE, &format!("{sha}:{refname}")])?;
    scan.heartbeats.insert(me.instance.to_string(), me.now);
    Ok(())
}

/// `afkconvoy heartbeat`: refresh my heartbeat if it is due.
pub fn beat(repo: &Repo, scan: &mut Scan, me: &Me) -> Result<Value> {
    let refname = heartbeat_ref(&me.instance);
    let last = scan.heartbeats.get(me.instance.as_str()).copied();
    if !afkconvoy_core::lease::heartbeat_due(last, me.now) {
        return Ok(json!({"refreshed": false, "reason": "not due", "ts": last, "ref": refname}));
    }
    push_heartbeat(repo, scan, me)?;
    Ok(json!({"refreshed": true, "ts": me.now, "ref": refname}))
}

/// How a claim ref is pushed in my name.
pub enum Take<'a> {
    /// Create it: the remote rejects a ref that already exists.
    Fresh,
    /// Re-stamp one that exists, only while it still points at this sha.
    From(&'a str),
}

/// Puts the claim on issue `number` in my name with one atomic push → whether
/// it was won. A lost race is an answer (`won: false`); a push that failed for
/// any other reason is an error.
pub fn take(repo: &Repo, scan: &mut Scan, me: &Me, number: u64, how: Take) -> Result<Value> {
    let refname = claim_ref(number);
    let owner = Owner { instance: me.instance.to_string(), host: me.host.clone(), ts: me.now };
    let sha = repo.record_commit(&owner.write())?;
    let last_beat = scan.heartbeats.get(me.instance.as_str()).copied();
    let mut pushed = None;
    for step in claim_steps(last_beat, me.now) {
        match step {
            ClaimStep::Beat => push_heartbeat(repo, scan, me)?,
            ClaimStep::Push => {
                let target = format!("{sha}:{refname}");
                let lease;
                let mut args = vec!["push", "--quiet", REMOTE];
                if let Take::From(expected) = how {
                    lease = format!("--force-with-lease={refname}:{expected}");
                    args.push(&lease);
                }
                args.push(&target);
                pushed = Some(repo.try_git(&args)?);
            }
        }
    }
    let pushed = pushed.expect("every claim order ends in the push");
    if pushed.ok {
        scan.written(number, Some(Claim { number, owner: Some(owner), sha: sha.clone() }));
        return Ok(json!({"won": true, "issue": number, "ref": refname, "sha": sha,
            "instance": me.instance.as_str()}));
    }
    // Lost only if somebody else's claim is where ours would be.
    let detail = pushed.stderr.trim().to_string();
    let there = repo.remote_records(&refname)?.pop();
    let moved = match (&there, &how) {
        (None, Take::Fresh) => false,
        (None, Take::From(_)) => true,
        (Some(_), Take::Fresh) => true,
        (Some(now), Take::From(expected)) => now.sha != *expected,
    };
    if !moved {
        bail!("the push of {refname} failed and this is not a lost race: {detail}");
    }
    let claim = there.map(|now| Claim { number, owner: Owner::read(&now.subject), sha: now.sha });
    let holder = claim.as_ref().map_or(Value::Null, claim_json);
    scan.written(number, claim);
    Ok(json!({"won": false, "issue": number, "ref": refname, "owner": holder, "detail": detail}))
}

/// Deletes one claim ref, only while it still points at `expected` — so a
/// claim somebody took meanwhile is never deleted from under them. Already
/// gone counts as released.
pub fn delete(repo: &Repo, scan: &mut Scan, number: u64, expected: &str) -> Result<Value> {
    let refname = claim_ref(number);
    let lease = format!("--force-with-lease={refname}:{expected}");
    let pushed = repo.try_git(&["push", "--quiet", REMOTE, &lease, &format!(":{refname}")])?;
    let now_at = if pushed.ok { None } else { repo.remote_sha(&refname)? };
    match now_at {
        None => {
            scan.written(number, None);
            Ok(json!({"released": true, "issue": number, "ref": refname}))
        }
        Some(sha) if sha == expected => {
            bail!("release failed and {refname} is still on the remote: {}", pushed.stderr.trim())
        }
        Some(sha) => bail!(
            "{refname} moved since it was read (expected {expected}, now {sha}): somebody took \
             the claim, which is no longer the caller's to release; it was left alone"
        ),
    }
}

/// `afkconvoy release`: delete my own claim on an issue.
pub fn release(repo: &Repo, scan: &mut Scan, me: &InstanceId, number: u64) -> Result<Value> {
    match releasable(scan.claim_of(number), me)? {
        Some(claim) => {
            let sha = claim.sha.clone();
            delete(repo, scan, number, &sha)
        }
        None => Ok(json!({"released": true, "issue": number, "ref": claim_ref(number)})),
    }
}

//! Claims and heartbeats on the remote's refs: the scan that reads them and
//! the pushes that write them.

use std::collections::BTreeMap;

use afkconvoy_core::claims::{
    CLAIM_KIND, Claim, ClaimStep, HEARTBEAT_KIND, InstanceId, OVER_KIND, Owner, RunClaim,
    claim_ref, claim_steps, heartbeat_ref, over_ref, read_heartbeat, read_over, releasable,
    run_ref, write_heartbeat, write_over,
};
use afkconvoy_core::layout::Layout;
use afkconvoy_core::patrol::{RUN_KIND, read_run};
use afkconvoy_core::record::fleet_number;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::git::{REMOTE, RemoteRecord, Repo};

/// Who is acting, and when.
pub struct Me {
    pub instance: InstanceId,
    pub host: String,
    pub now: u64,
}

/// Every claim and heartbeat the remote has, and what it records of the
/// patrols, as one fetch saw them. A write this process makes is carried into
/// it, so a read after a write sees it.
#[derive(Debug, Default)]
pub struct Scan {
    pub claims: Vec<Claim>,
    pub heartbeats: BTreeMap<String, u64>,
    /// The claims on runs — a patrol's, the proposal of a precedent: one for
    /// each still going.
    pub runs: Vec<RunClaim>,
    /// patrol → when its last recorded run started.
    pub last_runs: BTreeMap<String, u64>,
}

impl Scan {
    /// Reads every claim and heartbeat record off the remote.
    /// An error when the remote cannot be read: a fleet whose claims are
    /// unreadable must not look like one holding none.
    pub fn read(repo: &Repo) -> Result<Scan> {
        let layout = repo.layout();
        Ok(Scan::among(layout, repo.remote_records(&layout.every(""))?))
    }

    /// The claims, heartbeats and runs among `records`: the fleet's refs
    /// under `layout`, as one read of the remote saw them.
    pub fn among(layout: Layout, records: Vec<RemoteRecord>) -> Scan {
        let mut scan = Scan::default();
        // run key → the claim its worker declared over.
        let mut over: BTreeMap<String, String> = BTreeMap::new();
        for record in records {
            let Some(kept) = layout.kept(&record.name) else { continue };
            if let Some(name) = kept.strip_prefix(CLAIM_KIND) {
                if let Some(number) = fleet_number(name) {
                    scan.claims.push(Claim::read(number, &record.subject, record.sha));
                } else {
                    let owner = Owner::read(&record.subject);
                    let (key, sha) = (name.to_string(), record.sha);
                    scan.runs.push(RunClaim { key, owner, sha, over: false });
                }
            } else if let Some(instance) = kept.strip_prefix(HEARTBEAT_KIND)
                && let Some(ts) = read_heartbeat(&record.subject)
            {
                scan.heartbeats.insert(instance.to_string(), ts);
            } else if let Some(patrol) = kept.strip_prefix(RUN_KIND)
                && let Some(ts) = read_run(&record.subject)
            {
                scan.last_runs.insert(patrol.to_string(), ts);
            } else if let Some(key) = kept.strip_prefix(OVER_KIND)
                && let Some(claim) = read_over(&record.subject)
            {
                over.insert(key.to_string(), claim);
            }
        }
        for run in &mut scan.runs {
            run.over = over.get(&run.key) == Some(&run.sha);
        }
        scan.claims.sort_by_key(|claim| claim.number);
        scan
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
    let refname = heartbeat_ref(repo.layout(), &me.instance);
    repo.git(&["push", "--quiet", "--force", REMOTE, &format!("{sha}:{refname}")])?;
    scan.heartbeats.insert(me.instance.to_string(), me.now);
    Ok(())
}

/// `afkconvoy heartbeat`: refresh my heartbeat if it is due.
pub fn beat(repo: &Repo, scan: &mut Scan, me: &Me) -> Result<Value> {
    let refname = heartbeat_ref(repo.layout(), &me.instance);
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
    /// Take this one over, only while its ref still points where it was
    /// read: it stays the claim it was created as.
    From(&'a Claim),
}

/// What putting a claim ref in my name came to.
enum Put {
    Won {
        owner: Owner,
        sha: String,
        /// The record the ref now carries.
        subject: String,
    },
    /// Somebody else's claim is where mine would be: the ref as it is now,
    /// when it is there, and what git said.
    Lost { there: Option<RemoteRecord>, detail: String },
}

/// Puts the claim ref `refname` in my name with one atomic push, behind a
/// fresh heartbeat of mine. A push that failed for any reason but a lost race
/// is an error.
fn put(repo: &Repo, scan: &mut Scan, me: &Me, refname: &str, how: Take) -> Result<Put> {
    let owner = Owner { instance: me.instance.to_string(), host: me.host.clone(), ts: me.now };
    let subject = match how {
        Take::Fresh => owner.write(),
        Take::From(taken) => owner.write_over(taken),
    };
    let sha = repo.record_commit(&subject)?;
    let last_beat = scan.heartbeats.get(me.instance.as_str()).copied();
    let mut pushed = None;
    for step in claim_steps(last_beat, me.now) {
        match step {
            ClaimStep::Beat => push_heartbeat(repo, scan, me)?,
            ClaimStep::Push => {
                let target = format!("{sha}:{refname}");
                let lease;
                let mut args = vec!["push", "--quiet", REMOTE];
                if let Take::From(taken) = how {
                    lease = format!("--force-with-lease={refname}:{}", taken.sha);
                    args.push(&lease);
                }
                args.push(&target);
                pushed = Some(repo.try_git(&args)?);
            }
        }
    }
    let pushed = pushed.expect("every claim order ends in the push");
    if pushed.ok {
        return Ok(Put::Won { owner, sha, subject });
    }
    // Lost only if somebody else's claim is where ours would be.
    let detail = pushed.stderr.trim().to_string();
    let there = repo.remote_records(refname)?.pop();
    let moved = match (&there, &how) {
        (None, Take::Fresh) => false,
        (None, Take::From(_)) => true,
        (Some(_), Take::Fresh) => true,
        (Some(now), Take::From(taken)) => now.sha != taken.sha,
    };
    if !moved {
        bail!("the push of {refname} failed and this is not a lost race: {detail}");
    }
    Ok(Put::Lost { there, detail })
}

/// Puts the claim on issue `number` in my name with one atomic push → whether
/// it was won. A lost race is an answer (`won: false`); a push that failed for
/// any other reason is an error.
pub fn take(repo: &Repo, scan: &mut Scan, me: &Me, number: u64, how: Take) -> Result<Value> {
    let refname = claim_ref(repo.layout(), number);
    match put(repo, scan, me, &refname, how)? {
        Put::Won { sha, subject, .. } => {
            scan.written(number, Some(Claim::read(number, &subject, sha.clone())));
            Ok(json!({"won": true, "issue": number, "ref": refname, "sha": sha,
                "instance": me.instance.as_str()}))
        }
        Put::Lost { there, detail } => {
            let claim = there.map(|now| Claim::read(number, &now.subject, now.sha));
            let holder = claim.as_ref().map_or(Value::Null, claim_json);
            scan.written(number, claim);
            Ok(json!({"won": false, "issue": number, "ref": refname, "owner": holder,
                "detail": detail}))
        }
    }
}

/// Claims the run keyed `key` in my name → the claim, or `None` when a peer
/// holds one: of several hosts that found the run due, one makes it.
pub fn take_run(repo: &Repo, scan: &mut Scan, me: &Me, key: &str) -> Result<Option<RunClaim>> {
    let refname = run_ref(repo.layout(), key);
    scan.runs.retain(|run| run.key != key);
    let (claim, won) = match put(repo, scan, me, &refname, Take::Fresh)? {
        Put::Won { owner, sha, .. } => {
            (Some(RunClaim { key: key.to_string(), owner: Some(owner), sha, over: false }), true)
        }
        Put::Lost { there, .. } => {
            let read = |now: RemoteRecord| RunClaim {
                key: key.to_string(),
                owner: Owner::read(&now.subject),
                sha: now.sha,
                over: false,
            };
            (there.map(read), false)
        }
    };
    scan.runs.extend(claim.clone());
    Ok(claim.filter(|_| won))
}

/// Deletes the claim on a run, only while it is still `claim`, and with it
/// what its worker declared. Already gone counts as released.
pub fn drop_run(repo: &Repo, scan: &mut Scan, claim: &RunClaim) -> Result<()> {
    unput(repo, &run_ref(repo.layout(), &claim.key), &claim.sha)?;
    if claim.over {
        // What is left of it names a claim that is gone, and declares nothing.
        repo.try_git(&[
            "push",
            "--quiet",
            REMOTE,
            &format!(":{}", over_ref(repo.layout(), &claim.key)),
        ])?;
    }
    scan.runs.retain(|run| run.key != claim.key);
    Ok(())
}

/// `afkconvoy over`: records on the remote that the run keyed `key` is over,
/// as the claim on it stands now, and wakes the fleet: its next tick ends the
/// run. An error when no run is claimed under `key` — an issue's claim is no
/// run's: a stage ends with a verdict.
pub fn declare_over(repo: &Repo, key: &str) -> Result<Value> {
    if fleet_number(key).is_some() {
        bail!(
            "{key} is an issue, and no run: the stage of a worker on an issue ends with \
             `afkconvoy verdict`. Only a patrol's run and a proposal's are declared over"
        );
    }
    let layout = repo.layout();
    let Some(claim) = repo.remote_records(&run_ref(layout, key))?.pop() else {
        bail!("no run {key:?} is going: there is nothing to declare over");
    };
    let refname = over_ref(layout, key);
    let sha = repo.record_commit(&write_over(&claim.sha))?;
    repo.git(&["push", "--quiet", "--force", REMOTE, &format!("{sha}:{refname}")])?;
    let woke = crate::wake::wake(repo)?;
    Ok(json!({"over": true, "run": key, "ref": refname, "woke": woke["woke"]}))
}

/// Deletes the claim ref `refname`, only while it still points at `expected`
/// — so a claim somebody took meanwhile is never deleted from under them.
/// Already gone counts as released.
fn unput(repo: &Repo, refname: &str, expected: &str) -> Result<()> {
    let lease = format!("--force-with-lease={refname}:{expected}");
    let pushed = repo.try_git(&["push", "--quiet", REMOTE, &lease, &format!(":{refname}")])?;
    let now_at = if pushed.ok { None } else { repo.remote_sha(refname)? };
    match now_at {
        None => Ok(()),
        Some(sha) if sha == expected => {
            bail!("release failed and {refname} is still on the remote: {}", pushed.stderr.trim())
        }
        Some(sha) => bail!(
            "{refname} moved since it was read (expected {expected}, now {sha}): somebody took \
             the claim, which is no longer the caller's to release; it was left alone"
        ),
    }
}

/// Deletes the claim ref of issue `number`, only while it still points at
/// `expected` (`unput`).
pub fn delete(repo: &Repo, scan: &mut Scan, number: u64, expected: &str) -> Result<Value> {
    let refname = claim_ref(repo.layout(), number);
    unput(repo, &refname, expected)?;
    scan.written(number, None);
    Ok(json!({"released": true, "issue": number, "ref": refname}))
}

/// `afkconvoy release`: delete my own claim on an issue.
pub fn release(repo: &Repo, scan: &mut Scan, me: &InstanceId, number: u64) -> Result<Value> {
    match releasable(scan.claim_of(number), me)? {
        Some(claim) => {
            let sha = claim.sha.clone();
            delete(repo, scan, number, &sha)
        }
        None => {
            Ok(json!({"released": true, "issue": number, "ref": claim_ref(repo.layout(), number)}))
        }
    }
}

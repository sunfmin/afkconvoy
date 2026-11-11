//! Claims and heartbeats on the remote's refs: the scan that reads them and
//! the pushes that write them.

use std::collections::BTreeMap;

use afkconvoy_core::claims::{
    Beat, CLAIM_KIND, Claim, ClaimStep, HEARTBEAT_KIND, InstanceId, OVER_KIND, Owner, RunClaim,
    WAIT_KIND, claim_ref, claim_steps, heartbeat_ref, over_ref, read_heartbeat, read_over,
    releasable, run_ref, wait_ref, write_heartbeat, write_over,
};
use afkconvoy_core::layout::Layout;
use afkconvoy_core::patrol::{
    ASKED_KIND, RUN_KIND, Ran, asked_ref, read_asked, read_run, write_asked,
};
use afkconvoy_core::record::canonical_number;
use afkconvoy_core::takeover::{held, holds};
use afkconvoy_core::tracker::{Claimed, Tracker};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::git::Repo;
use crate::remote::{Fresh, Push, RemoteRecord};

/// Who is acting, and when.
pub struct Me {
    pub instance: InstanceId,
    pub host: String,
    pub now: u64,
}

/// Reads this issue's claim before a worker acts. Ownership follows the
/// host, so a restart here can keep its workers; a takeover elsewhere cannot.
/// An absent claim is left to the command's existing unclaimed semantics.
/// `fresh` is live unless the caller just listed the claim with its other refs.
pub fn on_this_host(repo: &Repo, issue: u64, host: &str, fresh: Fresh) -> Result<Option<Claim>> {
    let tracker = crate::checkout_list::tracker_of(repo)?;
    let name = claim_ref(repo.layout(), &tracker, issue);
    let claim = repo
        .remote()
        .records(&[&name], fresh)?
        .pop()
        .map(|record| Claim::read(issue, &record.subject, record.sha));
    if let Some(owner) = claim.as_ref().and_then(|claim| claim.owner.as_ref())
        && owner.host != host
    {
        bail!(
            "the claim on #{issue} is held by instance {} on host {} since {}: this worker's work was taken; stop",
            owner.instance,
            owner.host,
            owner.ts
        );
    }
    Ok(claim)
}

/// A standalone gate has no issue argument: the worker's environment names it.
pub fn own_issue_on_this_host(repo: &Repo) -> Result<()> {
    if let Some(issue) =
        std::env::var(afkconvoy_core::role::ISSUE_ENV).ok().and_then(|raw| canonical_number(&raw))
    {
        on_this_host(repo, issue, &crate::host_settings::host_name(), Fresh::Live)?;
    }
    Ok(())
}

/// Every claim and heartbeat the remote has, and what it records of the
/// patrols, as one fetch saw them. A write this process makes is carried into
/// it, so a read after a write sees it. Its claims on issues are those of the
/// issues of `tracker`: a claim on an issue of another tracker is another
/// fleet's, and is no claim here, on an issue or on a run.
#[derive(Debug)]
pub struct Scan {
    /// The tracker whose issues its claims are on, and which names the ref of
    /// each claim it writes.
    pub tracker: Tracker,
    pub claims: Vec<Claim>,
    /// Claims on human waits: neither workers nor patrol runs.
    pub waits: Vec<Claim>,
    pub heartbeats: BTreeMap<String, Beat>,
    /// The claims on runs — a patrol's or a cut's: one for
    /// each still going.
    pub runs: Vec<RunClaim>,
    /// patrol → its last recorded run.
    pub last_runs: BTreeMap<String, Ran>,
    /// patrol → a human's ask for a run of it now, while one stands.
    pub asks: BTreeMap<String, Ask>,
}

/// A human's ask for a run of a patrol now, as the remote has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// When it was made.
    pub ts: u64,
    /// The commit its ref points at: what deleting it leases on.
    pub sha: String,
}

impl Scan {
    /// Reads every claim and heartbeat record off the remote, the claims on
    /// issues those of the tracker the checkout `repo` says on this host.
    /// An error when the remote cannot be read: a fleet whose claims are
    /// unreadable must not look like one holding none.
    pub fn read(repo: &Repo) -> Result<Scan> {
        let (layout, tracker) = (repo.layout(), crate::checkout_list::tracker_of(repo)?);
        Ok(Scan::among(
            layout,
            tracker,
            repo.remote().records(&[&layout.every("")], Fresh::Process)?,
        ))
    }

    /// `read`, for a command an operator runs on the claims themselves —
    /// `scan`, `release`: where the checkout says no tracker at all, being on
    /// no checkout list of this host, and the remote holds claims on the
    /// issues of another tracker, an error saying the tracker is not said.
    /// Read as its own repo's, a claim named for that tracker would be
    /// reported absent, or released when it is not.
    pub fn read_said(repo: &Repo) -> Result<Scan> {
        let list = crate::checkout_list::List::of_host()?;
        if crate::checkout_list::said(&list, repo)?.is_some() {
            return Scan::read(repo);
        }
        let layout = repo.layout();
        let records = repo.remote().records(&[&layout.every("")], Fresh::Process)?;
        let elsewhere: Vec<&str> = records
            .iter()
            .filter_map(|record| layout.kept(&record.name)?.strip_prefix(CLAIM_KIND))
            .filter(|key| key.contains('/') && Tracker::Own.claimed(key) == Claimed::Elsewhere)
            .filter_map(|key| key.rsplit_once('/').map(|(tracker, _)| tracker))
            .collect();
        if let Some(tracker) = elsewhere.first() {
            bail!(
                "the tracker is not said: {} is on no checkout list of this host, and the remote \
                 holds claims on the issues of {tracker}, which are named for their tracker. Run \
                 it while the checkout is on the list — `afkconvoy run --tracker <owner>/<repo>` \
                 puts it there",
                repo.dir.display()
            );
        }
        Ok(Scan::among(layout, Tracker::Own, records))
    }

    /// The claims, heartbeats and runs among `records`: the fleet's refs
    /// under `layout`, as one read of the remote saw them, its claims on
    /// issues those of `tracker`.
    pub fn among(layout: Layout, tracker: Tracker, records: Vec<RemoteRecord>) -> Scan {
        let mut scan = Scan {
            tracker,
            claims: vec![],
            waits: vec![],
            heartbeats: BTreeMap::new(),
            runs: vec![],
            last_runs: BTreeMap::new(),
            asks: BTreeMap::new(),
        };
        // run key → the claim its worker declared over.
        let mut over: BTreeMap<String, String> = BTreeMap::new();
        for record in records {
            let Some(kept) = layout.kept(&record.name) else { continue };
            if let Some(name) = kept.strip_prefix(CLAIM_KIND) {
                match scan.tracker.claimed(name) {
                    Claimed::Issue(number) => {
                        scan.claims.push(Claim::read(number, &record.subject, record.sha));
                    }
                    Claimed::Elsewhere => {}
                    Claimed::Run => {
                        let owner = Owner::read(&record.subject);
                        let (key, sha) = (name.to_string(), record.sha);
                        scan.runs.push(RunClaim { key, owner, sha, over: false });
                    }
                }
            } else if let Some(key) = kept.strip_prefix(WAIT_KIND) {
                if let Claimed::Issue(number) = scan.tracker.claimed(key) {
                    scan.waits.push(Claim::read(number, &record.subject, record.sha));
                }
            } else if let Some(instance) = kept.strip_prefix(HEARTBEAT_KIND)
                && let Some(beat) = read_heartbeat(&record.subject, record.sha.clone())
            {
                scan.heartbeats.insert(instance.to_string(), beat);
            } else if let Some(patrol) = kept.strip_prefix(RUN_KIND)
                && let Some(ran) = read_run(&record.subject)
            {
                scan.last_runs.insert(patrol.to_string(), ran);
            } else if let Some(patrol) = kept.strip_prefix(ASKED_KIND)
                && let Some(ts) = read_asked(&record.subject)
            {
                scan.asks.insert(patrol.to_string(), Ask { ts, sha: record.sha });
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

    /// When `instance` last beat, as far as this scan knows.
    pub fn last_beat(&self, instance: &str) -> Option<u64> {
        self.heartbeats.get(instance).map(|beat| beat.ts)
    }

    /// instance → the time of its heartbeat, for every one the remote has.
    pub fn beats(&self) -> Vec<(String, u64)> {
        self.heartbeats.iter().map(|(instance, beat)| (instance.clone(), beat.ts)).collect()
    }

    fn written(&mut self, number: u64, claim: Option<Claim>) {
        self.claims.retain(|claim| claim.number != number);
        self.claims.extend(claim);
        self.claims.sort_by_key(|claim| claim.number);
    }

    pub fn json(&self) -> Value {
        json!({
            "claims": self.claims.iter().map(claim_json).collect::<Vec<_>>(),
            "heartbeats": self.beats().into_iter().collect::<BTreeMap<_, _>>(),
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
    let sha = repo.record_commit(&write_heartbeat(&me.instance, &me.host, me.now))?;
    let refname = heartbeat_ref(repo.layout(), &me.instance);
    repo.remote().push(&Push::set(&sha, &refname).force())?;
    let beat = Beat { ts: me.now, host: me.host.clone(), sha };
    scan.heartbeats.insert(me.instance.to_string(), beat);
    Ok(())
}

/// Deletes `instance`'s heartbeat ref, only while it still points where this
/// scan read it → whether it is gone. One that moved is its owner beating
/// again, and is left alone; so is one the push could not delete, which the
/// next start finds again.
pub fn drop_heartbeat(repo: &Repo, scan: &mut Scan, instance: &InstanceId) -> Result<bool> {
    let Some(beat) = scan.heartbeats.get(instance.as_str()) else { return Ok(true) };
    let refname = heartbeat_ref(repo.layout(), instance);
    let push = Push::delete(&refname).leased(&beat.sha);
    let pushed = repo.remote().try_push_retried(&push, None, || Ok(false))?;
    if pushed.ok {
        scan.heartbeats.remove(instance.as_str());
    }
    Ok(pushed.ok)
}

/// What a `run` told to stop leaves of `me` on the remote, read anew: holding
/// no claim — on an issue or on a run — it takes its heartbeat with it;
/// holding one, it leaves the heartbeat with the claims, which the next start
/// on this host takes (docs/adr/0003) → whether no heartbeat of it is left.
pub fn goodbye(repo: &Repo, me: &Me) -> Result<bool> {
    let mut scan = Scan::read(repo)?;
    if holds(&held(&scan.claims, &scan.runs), me.instance.as_str()) {
        return Ok(false);
    }
    if !drop_heartbeat(repo, &mut scan, &me.instance)? {
        bail!("the remote kept {}", heartbeat_ref(repo.layout(), &me.instance));
    }
    Ok(true)
}

/// `afkconvoy heartbeat`: refresh my heartbeat if it is due.
pub fn beat(repo: &Repo, scan: &mut Scan, me: &Me) -> Result<Value> {
    let refname = heartbeat_ref(repo.layout(), &me.instance);
    let last = scan.last_beat(me.instance.as_str());
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
    /// Take this claim on a run over, only while its ref still points where
    /// it was read. What its worker declared is carried on by whoever takes
    /// it (`retake_run`).
    Run(&'a RunClaim),
}

impl Take<'_> {
    /// Where the ref must still point for the push to be taken; `None`: it
    /// must not exist.
    fn leased_to(&self) -> Option<&str> {
        match self {
            Take::Fresh => None,
            Take::From(taken) => Some(&taken.sha),
            Take::Run(taken) => Some(&taken.sha),
        }
    }
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
        Take::Fresh | Take::Run(_) => owner.write(),
        Take::From(taken) => owner.write_over(taken),
    };
    let subject = crate::target::stamp_claim(repo, &scan.tracker, refname, subject)?;
    let sha = repo.record_commit(&subject)?;
    let last_beat = scan.last_beat(me.instance.as_str());
    // Mine if a refused attempt's earlier one took; else lost only if
    // somebody else's claim is where mine would be.
    let mine = |there: &Option<RemoteRecord>| there.as_ref().is_some_and(|now| now.sha == sha);
    let moved = |there: &Option<RemoteRecord>| match (there, how.leased_to()) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => true,
        (Some(now), Some(expected)) => now.sha != expected,
    };
    let mut pushed = None;
    for step in claim_steps(last_beat, me.now) {
        match step {
            ClaimStep::Beat => push_heartbeat(repo, scan, me)?,
            ClaimStep::Push => {
                let mut push = Push::set(&sha, refname);
                if let Some(expected) = how.leased_to() {
                    push = push.leased(expected);
                }
                // The remote is read again before each retry: a claim another
                // host's push took meanwhile is a lost race, not tried again.
                let settled = || {
                    let there = repo.remote().records(&[refname], Fresh::Live)?.pop();
                    Ok(mine(&there) || moved(&there))
                };
                pushed = Some(repo.remote().try_push_retried(&push, None, settled)?);
            }
        }
    }
    let pushed = pushed.expect("every claim order ends in the push");
    if pushed.ok {
        return Ok(Put::Won { owner, sha, subject });
    }
    let detail = pushed.stderr.trim().to_string();
    let there = repo.remote().records(&[refname], Fresh::Live)?.pop();
    if mine(&there) {
        return Ok(Put::Won { owner, sha, subject });
    }
    if !moved(&there) {
        bail!("the push of {refname} failed and this is not a lost race: {detail}");
    }
    Ok(Put::Lost { there, detail })
}

/// Puts the claim on issue `number` in my name with one atomic push → whether
/// it was won. A lost race is an answer (`won: false`); a push that failed for
/// any other reason is an error.
pub fn take(repo: &Repo, scan: &mut Scan, me: &Me, number: u64, how: Take) -> Result<Value> {
    let config = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    crate::target::pin(repo, &config, number, None)?;
    let refname = claim_ref(repo.layout(), &scan.tracker, number);
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

/// Takes a human wait with the same atomic push and heartbeat ordering as a
/// worker claim. Even a lost race updates the scan with the actual holder.
pub fn take_wait(repo: &Repo, scan: &mut Scan, me: &Me, number: u64, how: Take) -> Result<()> {
    let name = wait_ref(repo.layout(), &scan.tracker, number);
    let claim = match put(repo, scan, me, &name, how)? {
        Put::Won { sha, subject, .. } => Some(Claim::read(number, &subject, sha)),
        Put::Lost { there, .. } => {
            let claim = there.map(|r| Claim::read(number, &r.subject, r.sha));
            // The winner may have published its first heartbeat after our
            // scan. Carry that live evidence with the claim we just read.
            if let Some(owner) = claim.as_ref().and_then(|c| c.owner.as_ref()) {
                let instance = InstanceId::parse(&owner.instance).map_err(anyhow::Error::msg)?;
                let name = heartbeat_ref(repo.layout(), &instance);
                if let Some(record) = repo.remote().records(&[&name], Fresh::Live)?.pop()
                    && let Some(beat) = read_heartbeat(&record.subject, record.sha)
                {
                    scan.heartbeats.insert(owner.instance.clone(), beat);
                }
            }
            claim
        }
    };
    scan.waits.retain(|claim| claim.number != number);
    scan.waits.extend(claim);
    Ok(())
}

/// Ends a wait only while it is still the claim this look read.
pub fn drop_wait(repo: &Repo, scan: &mut Scan, claim: &Claim) -> Result<()> {
    unput(repo, &wait_ref(repo.layout(), &scan.tracker, claim.number), &claim.sha)?;
    scan.waits.retain(|c| c.number != claim.number);
    Ok(())
}

/// Puts the claim on the run keyed `key` in my name with one atomic push →
/// the claim, or `None` when it was lost to whoever holds it now.
fn put_run(
    repo: &Repo,
    scan: &mut Scan,
    me: &Me,
    key: &str,
    how: Take,
) -> Result<Option<RunClaim>> {
    let refname = run_ref(repo.layout(), key);
    scan.runs.retain(|run| run.key != key);
    let (claim, won) = match put(repo, scan, me, &refname, how)? {
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

/// Claims the run keyed `key` in my name → the claim, or `None` when a peer
/// holds one: of several hosts that found the run due, one makes it.
pub fn take_run(repo: &Repo, scan: &mut Scan, me: &Me, key: &str) -> Result<Option<RunClaim>> {
    put_run(repo, scan, me, key, Take::Fresh)
}

/// Re-stamps the claim on a run in my name, only while it is still `claim` →
/// whether it was won. A run its worker declared over is over under my claim
/// too: the worker is not there to say it again.
pub fn retake_run(repo: &Repo, scan: &mut Scan, me: &Me, claim: &RunClaim) -> Result<bool> {
    let Some(mine) = put_run(repo, scan, me, &claim.key, Take::Run(claim))? else {
        return Ok(false);
    };
    if claim.over {
        push_over(repo, &mine.key, &mine.sha)?;
        for run in scan.runs.iter_mut().filter(|run| run.key == mine.key) {
            run.over = true;
        }
    }
    Ok(true)
}

/// Records on the remote that the run keyed `key`, claimed at `claim`, is
/// over → the ref that says so.
fn push_over(repo: &Repo, key: &str, claim: &str) -> Result<String> {
    let refname = over_ref(repo.layout(), key);
    let sha = repo.record_commit(&write_over(claim))?;
    repo.remote().push(&Push::set(&sha, &refname).force())?;
    Ok(refname)
}

/// Deletes the claim on a run, only while it is still `claim`, and with it
/// what its worker declared. Already gone counts as released.
pub fn drop_run(repo: &Repo, scan: &mut Scan, claim: &RunClaim) -> Result<()> {
    unput(repo, &run_ref(repo.layout(), &claim.key), &claim.sha)?;
    if claim.over {
        // What is left of it names a claim that is gone, and declares nothing.
        let push = Push::delete(&over_ref(repo.layout(), &claim.key));
        repo.remote().try_push_retried(&push, None, || Ok(false))?;
    }
    scan.runs.retain(|run| run.key != claim.key);
    Ok(())
}

/// Records on the remote that a human on `host` asked at `now` for a run of
/// the patrol `name` now → the ref it is kept at. An ask standing already is
/// replaced: of two, one stands, and one run answers it.
pub fn push_ask(repo: &Repo, scan: &mut Scan, name: &str, host: &str, now: u64) -> Result<String> {
    let refname = asked_ref(repo.layout(), name);
    let sha = repo.record_commit(&write_asked(now, host))?;
    repo.remote().push(&Push::set(&sha, &refname).force())?;
    scan.asks.insert(name.to_string(), Ask { ts: now, sha });
    Ok(refname)
}

/// Deletes the ask `ask` for a run of the patrol `name`, which a run has
/// answered, only while it is still that one: an ask made since is another
/// human's, and stands. Already gone counts as deleted.
pub fn drop_ask(repo: &Repo, scan: &mut Scan, name: &str, ask: &Ask) -> Result<()> {
    let refname = asked_ref(repo.layout(), name);
    let push = Push::delete(&refname).leased(&ask.sha);
    let pushed = repo.remote().try_push_retried(&push, None, || Ok(false))?;
    if !pushed.ok
        && repo.remote().sha(&refname, Fresh::Process)?.as_deref() == Some(ask.sha.as_str())
    {
        bail!("the ask {refname} is still on the remote: {}", pushed.stderr.trim());
    }
    scan.asks.remove(name);
    Ok(())
}

/// `afkconvoy over`: records on the remote that the run keyed `key` is over,
/// as the claim on it stands now; the fleet is woken as the command ends
/// (`crate::acting`), and its next tick ends the run. An error when no run is claimed under `key` — an issue's claim is no
/// run's: a stage ends with a verdict.
pub fn declare_over(repo: &Repo, key: &str) -> Result<Value> {
    if canonical_number(key).is_some() {
        bail!(
            "{key} is an issue, and no run: the stage of a worker on an issue ends with \
             `afkconvoy verdict`. Only a patrol's run is declared over"
        );
    }
    let layout = repo.layout();
    let Some(claim) = repo.remote().records(&[&run_ref(layout, key)], Fresh::Process)?.pop() else {
        bail!("no run {key:?} is going: there is nothing to declare over");
    };
    let refname = push_over(repo, key, &claim.sha)?;
    Ok(json!({"over": true, "run": key, "ref": refname}))
}

/// Deletes the claim ref `refname`, only while it still points at `expected`
/// — so a claim somebody took meanwhile is never deleted from under them.
/// Already gone counts as released.
fn unput(repo: &Repo, refname: &str, expected: &str) -> Result<()> {
    let push = Push::delete(refname).leased(expected);
    let pushed = repo.remote().try_push_retried(&push, None, || Ok(false))?;
    let now_at = if pushed.ok { None } else { repo.remote().sha(refname, Fresh::Process)? };
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
    let refname = claim_ref(repo.layout(), &scan.tracker, number);
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
            let refname = claim_ref(repo.layout(), &scan.tracker, number);
            Ok(json!({"released": true, "issue": number, "ref": refname}))
        }
    }
}

/// What `afkconvoy heartbeat --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HEARTBEAT_HELP: &str = r#"## When to run it

Hardly ever: every tick refreshes its instance's heartbeat. Run it to keep the claims of an instance live while nothing ticks for it — claims made by hand, or a fleet stopped for a while whose claims no other host should take.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

It reads the instance's heartbeat on the remote and, when it is due — never written, or older than a third of the lease — writes it anew. Nothing on GitHub changes.

## What it answers

{"refreshed": true, "ts", "ref"} {"refreshed": false, "reason": "not due", "ts", "ref"} — ts is the heartbeat that stands, in epoch seconds.

Errors: the remote cannot be read, or refuses the push.

## What trips you up

- `--instance` is required.
- It keeps a dead fleet's claims alive: while they are beaten for, no other host takes them as stale, and nobody works them.
- One beat lasts one lease. Kept up by hand, it has to be run again before that."#;

/// What `afkconvoy release --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const RELEASE_HELP: &str = r#"## When to run it

Hardly ever: `close`, `escalate` and `park` release the claim they end, and a landed PR's claim is released by the tick. Run it to drop a claim made with `claim`, or one left behind by something that went wrong.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

It deletes the issue's claim on the remote, only while it is still the claim that was read. It does not read or change GitHub, and touches no worker and no worktree.

## What it answers

{"released": true, "issue", "ref"} — also when the issue had no claim: already gone counts as released.

Errors: the claim is another instance's ("is not this fleet's claim"); it "moved since it was read" — somebody took it meanwhile, and it was left alone; the delete failed; "the tracker is not said" — the checkout is on no checkout list of this host, and the remote holds claims named for another tracker, so the claim's ref cannot be named.

## What trips you up

- `--instance` is required, and must be the instance that holds the claim. To end another instance's claim, take it first: `takeover`.
- It only unlocks. The issue keeps its labels, its worker keeps working, and — still open and ready — the issue is back on the frontier: a fleet dispatches it again."#;

/// What `afkconvoy claim --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const CLAIM_HELP: &str = r#"## When to run it

Hardly ever: `dispatch` claims the issue it starts a worker on. Run it to hold an issue back from every fleet — a claim keeps the issue off the frontier — or to repair a claim that was released by mistake.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

It creates the issue's claim on the remote in one atomic push, which succeeds for exactly one instance. Before it, it refreshes the instance's heartbeat when that is due. It does not read GitHub: the issue is not checked, and no worker is started.

## What it answers

{"won": true, "issue", "ref", "sha", "instance"} {"won": false, "issue", "ref", "owner", "detail"} — the issue is claimed already. owner is that claim: {number, instance, host, ts, sha}.

Errors: the push failed and it was not a lost race; the remote cannot be read.

## What trips you up

- `--instance` is required. A lost race is an answer with exit 0: read `won`.
- A claim the instance holds already answers `won: false`, with itself as the owner.
- A claim is live only while its instance's heartbeat is within the lease. Under an id nothing beats for, another fleet takes it as stale once the lease runs out.
- Claimed as a running fleet's instance, it is a claim with no worker: that fleet's next tick starts a worker on it."#;

/// What `afkconvoy scan --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const SCAN_HELP: &str = r#"## When to run it

To see exactly what the remote holds: which instance claims which issue, and when each instance last beat. `plan` and `takeover --list` say what that means; this is the refs themselves, for repairing them.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

It reads the fleet's refs on the remote. It changes nothing, and does not read GitHub or the config.

## What it answers

{"claims", "heartbeats"}

- `claims`: one per claimed issue, lowest number first: {number, instance, host, ts, sha}. ts is when it was stamped, in epoch seconds; sha is what a take or a release of it is leased to
- `heartbeats`: {<instance id>: <epoch seconds>}

Errors: the remote cannot be read; "is not in a git checkout", or git's own words for a checkout it refuses; "the tracker is not said" — the checkout is on no checkout list of this host, and the remote holds claims named for another tracker, which would read as no claim.

## What trips you up

- It looks where this repo's last startup probe found the remote keeps the fleet's refs. In a clone no `plan`, `tick` or `run` was ever run in, it looks under refs/convoy/ — and finds nothing on a remote that keeps them as convoy-* branches.
- It does not say whether a claim is live: compare its instance's heartbeat with the lease, or read `takeover --list`."#;

/// What `afkconvoy over --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const OVER_HELP: &str = r#"## When to run it

Once, as the last command of a worker that is on no issue: a patrol's, when it has nothing more to open. The worker's brief ends with the exact line, the run named in it.

## Who runs it

A patrol's worker, in its own worktree: the checkout it was started in. Never a worker on an issue: its stage ends with a verdict (`afkconvoy verdict --help`).

## What it reads and writes

It reads the claim on the run off the remote and pushes one record that names it — a ref of the fleet's, `over/<run>` — then wakes the `run` of this host (`afkconvoy wake --help`). The next tick finds the run declared over and ends it at once, as it would one whose worker is gone (`end_patrol`: `afkconvoy tick --help`): a patrol's checkout is still checked for a commit and a PR. The slot the run held is free from that tick on.

## What it answers

{"over": true, "run", "ref", "woke"} — the record is on the remote; `woke` is whether a `run` on this host was told.

Errors: no run is claimed under that name — it was ended already, or never started; the name is an issue's number; the worker is on an issue (AFKCONVOY_ISSUE is in the environment); the remote cannot be read or does not take the push.

## What trips you up

- It ends nothing itself: the fleet ends the run, at its next tick. A worker that goes on working after it is cut short.
- A worker that never runs it loses nothing but time: the fleet ends the run once the worker is gone, or has been stopped for the grace period — five minutes in which the run holds a slot of the concurrency no issue can be worked in.
- The record names the claim it was read from: left behind, it says nothing of the next run of the same patrol."#;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use afkconvoy_core::claims::{InstanceId, Owner, heartbeat_ref, run_ref};
    use afkconvoy_core::layout::Layout;
    use afkconvoy_core::tracker::Tracker;

    use super::{Me, Scan, beat, take_run};
    use crate::git::Repo;
    use crate::remote::tests::{Fake, HELD};

    /// A checkout whose remote is `fake`: its record commits are real.
    fn checkout(fake: &Arc<Fake>) -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        assert!(crate::git::run(dir.path(), &["init", "-q"], &[]).unwrap().ok);
        let repo = Repo::on_wire(dir.path().to_path_buf(), fake.clone());
        (dir, repo)
    }

    fn me(instance: &str) -> Me {
        Me { instance: InstanceId::parse(instance).unwrap(), host: "here".into(), now: 1_000 }
    }

    fn scan() -> Scan {
        Scan::among(Layout::Hidden, Tracker::Own, vec![])
    }

    #[test]
    fn a_heartbeat_refused_for_a_held_ref_lock_is_written_on_a_later_attempt() {
        let fake = Fake::with(&[]);
        let (_dir, repo) = checkout(&fake);
        let me = me("a");
        let name = heartbeat_ref(Layout::Hidden, &me.instance);
        fake.refuses(&name, HELD);
        let beaten = beat(&repo, &mut scan(), &me).unwrap();
        assert_eq!(beaten["refreshed"], true);
        assert!(fake.sha(&name).is_some());
        assert_eq!(fake.pushes(), 2);
    }

    #[test]
    fn a_heartbeat_refused_otherwise_fails_with_what_the_remote_said() {
        let fake = Fake::with(&[]);
        let (_dir, repo) = checkout(&fake);
        let me = me("a");
        let name = heartbeat_ref(Layout::Hidden, &me.instance);
        fake.refuses(&name, " ! [remote rejected] x -> refs/convoy/heartbeat/a (protected)");
        let error = beat(&repo, &mut scan(), &me).unwrap_err().to_string();
        assert!(error.contains("(protected)"), "{error}");
        assert_eq!(fake.pushes(), 1);
    }

    #[test]
    fn a_claim_refused_for_a_held_ref_lock_is_won_on_a_later_attempt() {
        let fake = Fake::with(&[]);
        let (_dir, repo) = checkout(&fake);
        let name = run_ref(Layout::Hidden, "patrol-x");
        fake.refuses(&name, HELD);
        let won = take_run(&repo, &mut scan(), &me("a"), "patrol-x").unwrap().unwrap();
        assert_eq!(fake.sha(&name), Some(won.sha));
    }

    #[test]
    fn a_held_ref_lock_that_was_another_hosts_win_is_a_lost_claim() {
        let fake = Fake::with(&[]);
        let (_dir, repo) = checkout(&fake);
        let theirs = Owner { instance: "b".into(), host: "there".into(), ts: 999 };
        let theirs = repo.record_commit(&theirs.write()).unwrap();
        let name = run_ref(Layout::Hidden, "patrol-x");
        fake.refuses_taken(&name, HELD, &theirs);
        let mut scan = scan();
        assert!(take_run(&repo, &mut scan, &me("a"), "patrol-x").unwrap().is_none());
        assert_eq!(fake.sha(&name), Some(theirs.clone()));
        let held = scan.runs.iter().find(|run| run.key == "patrol-x").unwrap();
        assert_eq!(held.sha, theirs);
        // The heartbeat, then the one claim push: a lost race is not tried again.
        assert_eq!(fake.pushes(), 2);
    }

    #[test]
    fn a_claim_held_on_every_attempt_fails_with_what_the_remote_said() {
        let fake = Fake::with(&[]);
        let (_dir, repo) = checkout(&fake);
        let name = run_ref(Layout::Hidden, "patrol-x");
        for _ in 0..5 {
            fake.refuses(&name, HELD);
        }
        let error = take_run(&repo, &mut scan(), &me("a"), "patrol-x").unwrap_err().to_string();
        assert!(error.contains("not a lost race") && error.contains("(failed)"), "{error}");
        assert_eq!(fake.sha(&name), None);
    }
}

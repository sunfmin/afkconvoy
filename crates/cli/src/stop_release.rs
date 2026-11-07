//! `afkconvoy stop --release`: the drain an operator names (docs/adr/0003).
//! The fleet instance of a checkout gives back the claims it still has at
//! work — on issues it has not declared on, and on its patrols' runs — its
//! workers are ended, and only then does the checkout leave this host's
//! checkout list. Plain `stop` releases nothing.
//!
//! Its tracker is read while the checkout is still on the list, and kept for
//! this process once the plain stop has taken it off (`checkout_list::asked`):
//! a claim's ref is named for its tracker. The fleet instance is stopped
//! before anything is released and is not ticking after, so no tick takes an
//! issue back between its claim's delete and its worker's end. Every claim
//! is deleted in one atomic push, each only while it still points where it
//! was read: of two, either both are released, or neither is.

use afkconvoy_core::claims::{Claim, InstanceId, RunClaim, claim_ref, run_ref};
use afkconvoy_core::plan::Status;
use afkconvoy_core::tracker::Tracker;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::checkout_list::{self, Asked, Entry, List};
use crate::claims::{Me, goodbye};
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::orca::{Source, Worktree};
use crate::remote::{Fresh, Push};

/// What the fleet instance holds, as one read of the fleet saw it.
struct Held {
    /// Its claims on issues it has not declared on: what is released.
    issues: Vec<Claim>,
    /// Its claims on its patrols' runs, with each patrol: released too.
    runs: Vec<(String, RunClaim)>,
    /// A cut it holds: a publish, which no release ends.
    cut: Option<RunClaim>,
    /// What it holds that its next tick acts on — a declared verdict, an
    /// issue to settle, a run of another kind — each named, with why.
    kept: Vec<Value>,
}

/// `afkconvoy stop --release CHECKOUT`: releases what the fleet instance of
/// `checkout` — `named`, or the one this host's runner or its tick log says
/// — holds, ends those workers and takes the checkout off the list → the
/// answer. A cut it holds is an error, and nothing changes. Where it holds
/// what its next tick must act on, or a claim moved since it was read,
/// nothing is released and the checkout stays on the list, each named.
pub fn stop_release(checkout: &Repo, named: Option<InstanceId>, now: u64) -> Result<Value> {
    let list = List::of_host()?;
    let root = checkout.dir.clone();
    let Some(entry) = list.read()?.checkouts.into_iter().find(|entry| entry.checkout == root)
    else {
        bail!(
            "{} is not on this host's checkout list, so its tracker is not said: `stop --release` \
             releases the claims of a checkout on it, while its entry still says the tracker \
             they are named for",
            root.display()
        );
    };
    let tracker = checkout_list::tracker_of(checkout)?;
    let instance = match named {
        Some(instance) => instance,
        None => instance_of(checkout)?,
    };

    // While its fleet instance still ticks: what it holds that no release ends
    // is refused before anything stops.
    let held = read(checkout, &instance, now)?;
    if let Some(cut) = &held.cut {
        bail!(
            "the fleet instance {instance} holds the cut ({}): a cut is a publish, which a \
             release does not end. Nothing was released, and {} stays on the checkout list: \
             `afkconvoy stop --release` again once the cut is over",
            run_ref(checkout.layout(), &cut.key),
            root.display()
        );
    }
    if !held.kept.is_empty() {
        return Ok(kept(&root, &instance, held.kept, None));
    }

    // Stopped, it ticks no more: off the list, its tracker kept for this
    // process from its entry.
    let stopped = crate::host_runner::stop(checkout)?;
    checkout_list::asked().insert(root.clone(), asked_of(&entry));
    let answer = released(checkout, &tracker, &instance, &entry, &list, &stopped, now);
    checkout_list::forget(&root);
    answer
}

/// What `stop --release` comes to once the fleet instance has stopped: every
/// claim it holds released at once, or — something it holds is one to keep,
/// or moved since it was read — none, and the checkout back on the list.
fn released(
    checkout: &Repo,
    tracker: &Tracker,
    instance: &InstanceId,
    entry: &Entry,
    list: &List,
    stopped: &Value,
    now: u64,
) -> Result<Value> {
    let root = &checkout.dir;
    let layout = checkout.layout();
    let back = || -> Result<()> {
        list.change(|listed| {
            if !listed.checkouts.iter().any(|on| on.checkout == entry.checkout) {
                listed.checkouts.push(entry.clone());
            }
        })
    };
    let (mut fleet, held) = match read_fleet(checkout, instance, now) {
        Ok(read) => read,
        Err(error) => {
            back()?;
            return Err(error.context(format!(
                "the fleet instance {instance} stopped, nothing was released, and {} is back on \
                 the checkout list",
                root.display()
            )));
        }
    };
    let mut keep = held.kept;
    if let Some(cut) = &held.cut {
        keep.push(json!({"claim": run_ref(layout, &cut.key), "run": cut.key,
            "why": "a cut is a publish, which a release does not end"}));
    }
    if !keep.is_empty() {
        back()?;
        return Ok(kept(root, instance, keep, Some(stopped)));
    }

    let leases: Vec<(String, String)> = held
        .issues
        .iter()
        .map(|claim| (claim_ref(layout, tracker, claim.number), claim.sha.clone()))
        .chain(held.runs.iter().map(|(_, run)| (run_ref(layout, &run.key), run.sha.clone())))
        .collect();
    let moved = unput_all(checkout, &leases)?;
    if !moved.is_empty() {
        back()?;
        return Ok(kept(root, instance, moved, Some(stopped)));
    }

    // The claims are gone, and the fleet instance ticks no more: each worker
    // is ended now, and no tick takes its issue back.
    let source = Source::of(checkout);
    let mut released = vec![];
    for claim in &held.issues {
        let refname = claim_ref(layout, tracker, claim.number);
        let ended = source.as_ref().map_err(|e| anyhow::anyhow!("{e:#}")).and_then(|source| {
            let worktree = Worktree::of_issue(source, claim.number)?;
            end_issue_worker(&mut fleet, claim.number, worktree)
        });
        released.push(with_worker(json!({"issue": claim.number, "claim": refname}), ended));
    }
    for (patrol, run) in &held.runs {
        let refname = run_ref(layout, &run.key);
        if run.over {
            // What its worker declared names a claim that is gone.
            let over = afkconvoy_core::claims::over_ref(layout, &run.key);
            let _ = checkout.remote().try_push_retried(&Push::delete(&over), None, || Ok(false));
        }
        let ended = source.as_ref().map_err(|e| anyhow::anyhow!("{e:#}")).and_then(|source| {
            let worktree = Worktree::named(source, &afkconvoy_core::patrol::checkout_name(patrol))?;
            end_run_worker(&fleet, worktree)
        });
        released.push(with_worker(json!({"patrol": patrol, "claim": refname}), ended));
    }

    // Holding nothing, it takes its heartbeat with it.
    let me = Me { instance: instance.clone(), host: crate::this_host(), now };
    let heartbeat = goodbye(checkout, &me).map_err(|error| format!("{error:#}"));
    Ok(json!({
        "checkout": root,
        "repo": stopped["repo"],
        "instance": instance.as_str(),
        "released": released,
        "kept": [],
        "removed": true,
        "heartbeat_removed": heartbeat.as_ref().is_ok_and(|gone| *gone),
        "heartbeat_error": heartbeat.err(),
        "stop": stopped["stop"],
        "host_runner": stopped["host_runner"],
    }))
}

/// The answer where nothing is released: each of `kept` named, and the
/// checkout on the list — where `stopped` says its fleet instance stopped,
/// back on it, for the next `run` to start.
fn kept(
    root: &std::path::Path,
    instance: &InstanceId,
    kept: Vec<Value>,
    stopped: Option<&Value>,
) -> Value {
    let said = match stopped {
        None => "nothing was released and the checkout stays on the checkout list: its fleet \
                 instance acts on each kept claim at its next tick; `afkconvoy stop --release` \
                 again after"
            .to_string(),
        Some(_) => "nothing was released and the checkout is back on the checkout list: its \
                    fleet instance stopped, and `afkconvoy run` in the checkout starts it again"
            .to_string(),
    };
    json!({
        "checkout": root,
        "repo": stopped.map_or(Value::Null, |stopped| stopped["repo"].clone()),
        "instance": instance.as_str(),
        "released": [],
        "kept": kept,
        "removed": false,
        "said": said,
        "stop": stopped.map_or(Value::Null, |stopped| stopped["stop"].clone()),
        "host_runner": stopped.map_or(Value::Null, |stopped| stopped["host_runner"].clone()),
    })
}

/// What the run of `checkout` asked, as its `entry` on the list says it.
fn asked_of(entry: &Entry) -> Asked {
    Asked { concurrency: entry.concurrency, tracker: entry.tracker.clone(), harness: entry.harness }
}

/// The fleet instance of `checkout` on this host: the one its host runner
/// holds, else the newest its tick log names of this host.
fn instance_of(checkout: &Repo) -> Result<InstanceId> {
    let held = crate::host_runner::holding()?
        .and_then(|holding| holding.instances.get(&checkout.dir).cloned().flatten());
    let logged = || {
        let entries = crate::log::Log::of(checkout).ok()?.read()?;
        crate::run_log::run(&entries, &crate::this_host()).0
    };
    match held.or_else(logged) {
        Some(instance) => InstanceId::parse(&instance).map_err(|why| anyhow::anyhow!("{why}")),
        None => bail!(
            "no fleet instance of {} is known on this host: name the one whose claims to \
             release, `--instance <id>`",
            checkout.dir.display()
        ),
    }
}

/// What `instance` holds, read anew.
fn read(checkout: &Repo, instance: &InstanceId, now: u64) -> Result<Held> {
    read_fleet(checkout, instance, now).map(|(_, held)| held)
}

/// The fleet's view, and what `instance` holds in it.
fn read_fleet(checkout: &Repo, instance: &InstanceId, now: u64) -> Result<(Fleet, Held)> {
    let (fleet, open) = Fleet::read(checkout, now)?;
    let (set, ..) = fleet.rebuild(&open, Some(instance))?;
    let layout = checkout.layout();
    let tracker = &fleet.scan.tracker;
    let mut held = Held { issues: vec![], runs: vec![], cut: None, kept: vec![] };
    for claim in fleet.scan.claims.iter().filter(|claim| claim.is_held_by(instance)) {
        let refname = claim_ref(layout, tracker, claim.number);
        let why = match set.mine.iter().find(|mine| mine.number == claim.number) {
            None => Some("its issue could not be read"),
            Some(mine) if mine.declared => {
                Some("its worker declared a verdict: its next tick acts on it")
            }
            Some(mine) => match mine.status {
                Status::Landed | Status::Closed | Status::Gone => {
                    Some("its issue is settled at its next tick")
                }
                Status::NoPr | Status::Landing | Status::AtStage => None,
            },
        };
        match why {
            Some(why) => {
                held.kept.push(json!({"issue": claim.number, "claim": refname, "why": why}))
            }
            None => held.issues.push(claim.clone()),
        }
    }
    let mine =
        |run: &&RunClaim| run.owner.as_ref().is_some_and(|o| o.instance == instance.as_str());
    for run in fleet.scan.runs.iter().filter(mine) {
        if run.key == afkconvoy_core::cut::CLAIM_KEY {
            held.cut = Some(run.clone());
        } else if let Some(patrol) = afkconvoy_core::patrol::claimed(&run.key) {
            held.runs.push((patrol.to_string(), run.clone()));
        } else {
            held.kept.push(json!({"run": run.key, "claim": run_ref(layout, &run.key),
                "why": "a run its next tick ends"}));
        }
    }
    Ok((fleet, held))
}

/// Deletes every ref of `leases` in one atomic push, each only while it
/// still points at its sha → the ones that moved since they were read, each
/// named; where any did, none was deleted. A push that failed with none
/// moved is an error.
pub fn unput_all(checkout: &Repo, leases: &[(String, String)]) -> Result<Vec<Value>> {
    if leases.is_empty() {
        return Ok(vec![]);
    }
    let push = leases.iter().fold(Push::default().atomic(), |push, (refname, sha)| {
        push.and_delete(refname).leased(sha)
    });
    let pushed = checkout.remote().try_push_retried(&push, None, || Ok(false))?;
    if pushed.ok {
        return Ok(vec![]);
    }
    let mut moved = vec![];
    for (refname, sha) in leases {
        let now_at = checkout.remote().sha(refname, Fresh::Process)?;
        if now_at.as_deref() != Some(sha.as_str()) {
            let why = match &now_at {
                Some(now_at) => {
                    format!("moved since it was read (from {sha} to {now_at}): it was left alone")
                }
                None => format!("was gone since it was read at {sha}"),
            };
            moved.push(json!({"claim": refname, "why": why}));
        }
    }
    if moved.is_empty() {
        bail!(
            "the release of the fleet instance's claims failed, and every one is still on the \
             remote: {}",
            pushed.stderr.trim()
        );
    }
    Ok(moved)
}

/// What became of a released claim's worker, laid into `released`: its
/// worktree, and whether it was removed or why it was kept.
fn with_worker(mut released: Value, ended: Result<Option<(Worktree, Option<String>)>>) -> Value {
    match ended {
        Ok(None) => released["worktree"] = Value::Null,
        Ok(Some((worktree, kept))) => {
            released["worktree"] = json!(worktree.path);
            released["worktree_removed"] = json!(kept.is_none());
            released["worktree_kept"] = json!(kept);
        }
        Err(error) => released["worker_error"] = json!(format!("{error:#}")),
    }
    released
}

/// Ends the worker of issue `number` in `worktree`: its terminal is closed,
/// and the worktree removed where it is clean at its target → the worktree,
/// with why it was kept.
fn end_issue_worker(
    fleet: &mut Fleet,
    number: u64,
    worktree: Option<Worktree>,
) -> Result<Option<(Worktree, Option<String>)>> {
    let Some(worktree) = worktree else { return Ok(None) };
    worktree.close_terminals()?;
    let kept = match fleet.task_tip(number) {
        Ok(target) => kept_why(&worktree, &target)?,
        Err(error) => Some(format!("its target could not be read: {error:#}")),
    };
    if kept.is_none() {
        worktree.remove()?;
    }
    Ok(Some((worktree, kept)))
}

/// Ends the worker of a patrol's run in `checkout`, as a run's end does: its
/// checkout is removed, its terminal with it — but one that left a commit
/// past the base branch or a change is kept, with its terminal closed.
fn end_run_worker(
    fleet: &Fleet,
    checkout: Option<Worktree>,
) -> Result<Option<(Worktree, Option<String>)>> {
    let Some(checkout) = checkout else { return Ok(None) };
    let kept = kept_why(&checkout, &fleet.base_tip()?)?;
    match kept {
        Some(_) => checkout.close_terminals()?,
        None => {
            checkout.close_terminals()?;
            checkout.remove()?;
        }
    }
    Ok(Some((checkout, kept)))
}

/// Why `worktree` is kept: it holds a commit past `target`, or an
/// uncommitted change; `None` where it is clean at it.
fn kept_why(worktree: &Worktree, target: &str) -> Result<Option<String>> {
    let repo = Repo::new(worktree.path.clone());
    let tip = worktree.tip()?;
    if !repo.within(&tip, target)? {
        return Ok(Some(format!("it holds a commit past its target, {tip}")));
    }
    if !repo.git_raw(&["status", "--porcelain"])?.trim().is_empty() {
        return Ok(Some("it holds an uncommitted change".to_string()));
    }
    Ok(None)
}
